//! Byte-fed ANSI/VT escape-sequence tokenizer.
//!
//! A small state machine modeled on the classic DEC-compatible parser
//! (ground / escape / CSI / OSC / DCS states). It is *incremental*: bytes can
//! arrive in arbitrary chunks — an escape sequence or a UTF-8 code point split
//! across two `feed` calls is reassembled correctly. Unknown or malformed
//! sequences are consumed and dropped rather than printed, which is exactly
//! what real terminals do and what keeps snapshots free of garbage.

/// One CSI parameter. Sub-parameters separated by `:` (e.g. `38:5:196`) are
/// kept together so SGR color forms survive intact instead of being smeared
/// into unrelated attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param(pub Vec<u16>);

impl Param {
    /// The primary value, `0` when the parameter was empty.
    pub fn value(&self) -> u16 {
        self.0.first().copied().unwrap_or(0)
    }
}

/// A decoded terminal action, ready for [`crate::screen::Screen::apply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// A printable character (already UTF-8 decoded).
    Print(char),
    /// A C0 control byte (BS, HT, LF, CR, ...).
    Control(u8),
    /// A CSI sequence: `ESC [ <private?> <params> <intermediate?> <final>`.
    Csi {
        private: Option<char>,
        params: Vec<Param>,
        intermediate: Option<char>,
        final_byte: char,
    },
    /// A non-CSI escape: `ESC <intermediate?> <final>` (e.g. `ESC 7`, `ESC M`).
    Esc {
        intermediate: Option<char>,
        final_byte: char,
    },
    /// An OSC string (window title etc.). Carried for completeness; the
    /// screen ignores it, so titles never pollute the grid.
    Osc(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    Osc,
    /// DCS / SOS / PM / APC — consumed until ST, produced as nothing.
    OpaqueString,
}

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const MAX_OSC: usize = 4096;
const MAX_PARAMS: usize = 32;

/// Incremental escape-sequence parser. Feed it bytes, collect [`Action`]s.
pub struct Parser {
    state: State,
    utf8: Vec<u8>,
    utf8_need: usize,
    private: Option<char>,
    intermediate: Option<char>,
    params: Vec<Param>,
    cur: Vec<u16>,
    cur_seen: bool,
    osc: String,
    osc_esc: bool,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub fn new() -> Self {
        Parser {
            state: State::Ground,
            utf8: Vec::new(),
            utf8_need: 0,
            private: None,
            intermediate: None,
            params: Vec::new(),
            cur: Vec::new(),
            cur_seen: false,
            osc: String::new(),
            osc_esc: false,
        }
    }

    /// Feed a chunk of bytes and append decoded actions to `out`.
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Action>) {
        for &b in bytes {
            self.step(b, out);
        }
    }

    /// Convenience: parse a complete byte buffer in one call.
    pub fn parse(bytes: &[u8]) -> Vec<Action> {
        let mut p = Parser::new();
        let mut out = Vec::new();
        p.feed(bytes, &mut out);
        out
    }

    fn step(&mut self, b: u8, out: &mut Vec<Action>) {
        match self.state {
            State::Ground => self.step_ground(b, out),
            State::Escape => self.step_escape(b, out),
            State::EscapeIntermediate => self.step_escape_intermediate(b, out),
            State::Csi => self.step_csi(b, out),
            State::Osc => self.step_osc(b, out),
            State::OpaqueString => self.step_opaque(b),
        }
    }

    fn step_ground(&mut self, b: u8, out: &mut Vec<Action>) {
        // Mid-UTF-8 continuation handling first.
        if self.utf8_need > 0 {
            if b & 0xc0 == 0x80 {
                self.utf8.push(b);
                self.utf8_need -= 1;
                if self.utf8_need == 0 {
                    if let Ok(s) = std::str::from_utf8(&self.utf8) {
                        if let Some(c) = s.chars().next() {
                            out.push(Action::Print(c));
                        }
                    } else {
                        out.push(Action::Print('\u{fffd}'));
                    }
                    self.utf8.clear();
                }
                return;
            }
            // Broken sequence: emit a replacement char, reprocess this byte.
            self.utf8.clear();
            self.utf8_need = 0;
            out.push(Action::Print('\u{fffd}'));
        }
        match b {
            ESC => self.enter_escape(),
            0x00..=0x1f | 0x7f => {
                // BEL, NUL and DEL do nothing visible; keep the rest.
                if !matches!(b, BEL | 0x00 | 0x7f) {
                    out.push(Action::Control(b));
                }
            }
            0x20..=0x7e => out.push(Action::Print(b as char)),
            0xc2..=0xdf => self.start_utf8(b, 1),
            0xe0..=0xef => self.start_utf8(b, 2),
            0xf0..=0xf4 => self.start_utf8(b, 3),
            _ => out.push(Action::Print('\u{fffd}')),
        }
    }

    fn start_utf8(&mut self, b: u8, need: usize) {
        self.utf8.clear();
        self.utf8.push(b);
        self.utf8_need = need;
    }

    fn enter_escape(&mut self) {
        self.state = State::Escape;
        self.private = None;
        self.intermediate = None;
        self.params.clear();
        self.cur.clear();
        self.cur_seen = false;
    }

    fn step_escape(&mut self, b: u8, out: &mut Vec<Action>) {
        match b {
            b'[' => self.state = State::Csi,
            b']' => {
                self.state = State::Osc;
                self.osc.clear();
                self.osc_esc = false;
            }
            // DCS, SOS, PM, APC: opaque strings terminated by ST.
            b'P' | b'X' | b'^' | b'_' => {
                self.state = State::OpaqueString;
                self.osc_esc = false;
            }
            // Intermediate bytes (charset designation like `ESC ( B`).
            0x20..=0x2f => {
                self.intermediate = Some(b as char);
                self.state = State::EscapeIntermediate;
            }
            ESC => self.enter_escape(),
            0x30..=0x7e => {
                out.push(Action::Esc {
                    intermediate: None,
                    final_byte: b as char,
                });
                self.state = State::Ground;
            }
            _ => self.state = State::Ground,
        }
    }

    fn step_escape_intermediate(&mut self, b: u8, out: &mut Vec<Action>) {
        match b {
            0x20..=0x2f => { /* extra intermediates: keep the first, eat the rest */ }
            0x30..=0x7e => {
                out.push(Action::Esc {
                    intermediate: self.intermediate,
                    final_byte: b as char,
                });
                self.state = State::Ground;
            }
            ESC => self.enter_escape(),
            _ => self.state = State::Ground,
        }
    }

    fn step_csi(&mut self, b: u8, out: &mut Vec<Action>) {
        match b {
            b'0'..=b'9' => {
                if self.cur.is_empty() {
                    self.cur.push(0);
                }
                let last = self.cur.last_mut().expect("just pushed");
                *last = last.saturating_mul(10).saturating_add(u16::from(b - b'0'));
                self.cur_seen = true;
            }
            b':' => {
                self.cur.push(0);
                self.cur_seen = true;
            }
            b';' => self.finish_param(),
            b'<' | b'=' | b'>' | b'?' => {
                // Private markers are only valid before any parameter.
                if self.params.is_empty() && !self.cur_seen {
                    self.private = Some(b as char);
                }
            }
            0x20..=0x2f => self.intermediate = Some(b as char),
            0x40..=0x7e => {
                self.finish_param();
                out.push(Action::Csi {
                    private: self.private,
                    params: std::mem::take(&mut self.params),
                    intermediate: self.intermediate,
                    final_byte: b as char,
                });
                self.state = State::Ground;
            }
            ESC => self.enter_escape(),
            // C0 inside CSI is executed by real terminals; keep it simple and
            // honor the common ones so `\r` inside a torn sequence still lands.
            0x00..=0x1f | 0x7f => {
                if !matches!(b, BEL | 0x00 | 0x7f) {
                    out.push(Action::Control(b));
                }
            }
            _ => self.state = State::Ground,
        }
    }

    fn finish_param(&mut self) {
        if self.params.len() < MAX_PARAMS {
            self.params.push(Param(std::mem::take(&mut self.cur)));
        } else {
            self.cur.clear();
        }
        self.cur_seen = false;
    }

    fn step_osc(&mut self, b: u8, out: &mut Vec<Action>) {
        if self.osc_esc {
            self.osc_esc = false;
            if b == b'\\' {
                out.push(Action::Osc(std::mem::take(&mut self.osc)));
                self.state = State::Ground;
                return;
            }
            // ESC followed by anything else restarts sequence parsing.
            self.enter_escape();
            self.step(b, out);
            return;
        }
        match b {
            BEL => {
                out.push(Action::Osc(std::mem::take(&mut self.osc)));
                self.state = State::Ground;
            }
            ESC => self.osc_esc = true,
            _ => {
                if self.osc.len() < MAX_OSC {
                    self.osc.push(b as char);
                }
            }
        }
    }

    fn step_opaque(&mut self, b: u8) {
        if self.osc_esc {
            self.osc_esc = false;
            if b == b'\\' {
                self.state = State::Ground;
            }
            return;
        }
        match b {
            ESC => self.osc_esc = true,
            BEL => self.state = State::Ground,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csi(actions: &[Action]) -> Vec<&Action> {
        actions
            .iter()
            .filter(|a| matches!(a, Action::Csi { .. }))
            .collect()
    }

    #[test]
    fn plain_text_prints_and_bel_is_dropped() {
        let a = Parser::parse(b"hi");
        assert_eq!(a, vec![Action::Print('h'), Action::Print('i')]);
        let a = Parser::parse(b"a\x07\r\nb");
        assert_eq!(
            a,
            vec![
                Action::Print('a'),
                Action::Control(b'\r'),
                Action::Control(b'\n'),
                Action::Print('b'),
            ]
        );
    }

    #[test]
    fn csi_params_are_decoded_with_empty_defaulting_to_zero() {
        let a = Parser::parse(b"\x1b[2;5H");
        match &a[0] {
            Action::Csi {
                params, final_byte, ..
            } => {
                assert_eq!(*final_byte, 'H');
                assert_eq!(params[0].value(), 2);
                assert_eq!(params[1].value(), 5);
            }
            other => panic!("expected CSI, got {other:?}"),
        }
        let a = Parser::parse(b"\x1b[;5H");
        match &a[0] {
            Action::Csi { params, .. } => {
                assert_eq!(params[0].value(), 0);
                assert_eq!(params[1].value(), 5);
            }
            other => panic!("expected CSI, got {other:?}"),
        }
    }

    #[test]
    fn colon_subparams_stay_grouped() {
        // Kitty/xterm truecolor form: 38:2:255:128:0.
        let a = Parser::parse(b"\x1b[38:2:255:128:0m");
        match &a[0] {
            Action::Csi { params, .. } => {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].0, vec![38, 2, 255, 128, 0]);
            }
            other => panic!("expected CSI, got {other:?}"),
        }
    }

    #[test]
    fn private_marker_is_captured() {
        let a = Parser::parse(b"\x1b[?25l");
        match &a[0] {
            Action::Csi {
                private,
                params,
                final_byte,
                ..
            } => {
                assert_eq!(*private, Some('?'));
                assert_eq!(params[0].value(), 25);
                assert_eq!(*final_byte, 'l');
            }
            other => panic!("expected CSI, got {other:?}"),
        }
    }

    #[test]
    fn sequence_split_across_feeds_is_reassembled() {
        let mut p = Parser::new();
        let mut out = Vec::new();
        p.feed(b"\x1b[3", &mut out);
        assert!(out.is_empty(), "incomplete CSI must not emit anything");
        p.feed(b"1mA", &mut out);
        assert_eq!(csi(&out).len(), 1);
        assert_eq!(*out.last().unwrap(), Action::Print('A'));
    }

    #[test]
    fn utf8_split_across_feeds_and_invalid_utf8() {
        let mut p = Parser::new();
        let mut out = Vec::new();
        let bytes = "é".as_bytes();
        p.feed(&bytes[..1], &mut out);
        assert!(out.is_empty());
        p.feed(&bytes[1..], &mut out);
        assert_eq!(out, vec![Action::Print('é')]);
        // A truncated 2-byte sequence becomes a replacement char.
        let a = Parser::parse(&[0xc3, b'x']);
        assert_eq!(a, vec![Action::Print('\u{fffd}'), Action::Print('x')]);
    }

    #[test]
    fn osc_terminated_by_bel_or_st() {
        let a = Parser::parse(b"\x1b]0;my title\x07after");
        assert_eq!(a[0], Action::Osc("0;my title".into()));
        assert_eq!(a[1], Action::Print('a'));
        let a = Parser::parse(b"\x1b]2;t\x1b\\x");
        assert_eq!(a[0], Action::Osc("2;t".into()));
        assert_eq!(a[1], Action::Print('x'));
    }

    #[test]
    fn dcs_is_consumed_silently() {
        let a = Parser::parse(b"\x1bPq#0\x1b\\ok");
        assert_eq!(a, vec![Action::Print('o'), Action::Print('k')]);
    }

    #[test]
    fn esc_dispatch_with_intermediate() {
        // Charset designation `ESC ( B` must not print anything.
        let a = Parser::parse(b"\x1b(Bz");
        assert_eq!(
            a,
            vec![
                Action::Esc {
                    intermediate: Some('('),
                    final_byte: 'B'
                },
                Action::Print('z'),
            ]
        );
    }

    #[test]
    fn esc_inside_csi_restarts_the_sequence() {
        // A torn escape must not corrupt the one that follows it.
        let a = Parser::parse(b"\x1b[12\x1b[31mX");
        let c = csi(&a);
        assert_eq!(c.len(), 1);
        match c[0] {
            Action::Csi {
                params, final_byte, ..
            } => {
                assert_eq!(*final_byte, 'm');
                assert_eq!(params[0].value(), 31);
            }
            other => panic!("expected CSI, got {other:?}"),
        }
        // Oversized parameters saturate instead of panicking.
        let a = Parser::parse(b"\x1b[99999999999999999999A");
        match &a[0] {
            Action::Csi { params, .. } => assert_eq!(params[0].value(), u16::MAX),
            other => panic!("expected CSI, got {other:?}"),
        }
    }
}
