//! The `.snap` file format: a reviewable, diffable text serialization of a
//! rendered screen.
//!
//! Design goals: a snapshot must read well in a pull request (`|`-prefixed
//! rows, styles as English words), parse back losslessly, and fail with a
//! *positioned* error on corruption instead of comparing garbage. The format
//! is versioned by its first line so future revisions stay detectable.

use crate::screen::{char_width, Screen};
use crate::style::Style;
use std::fmt;

/// A contiguous run of one non-default style on one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub row: usize,
    pub start: usize,
    /// Inclusive end column.
    pub end: usize,
    pub style: Style,
}

/// A rendered screen, decoupled from the live emulator: row texts (trailing
/// blanks trimmed) plus style spans. Two frames are visually identical iff
/// they are `==`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub cols: usize,
    pub rows: usize,
    pub lines: Vec<String>,
    pub spans: Vec<Span>,
}

impl Frame {
    pub fn from_screen(screen: &Screen) -> Frame {
        let mut lines = Vec::with_capacity(screen.rows());
        let mut spans = Vec::new();
        for row in 0..screen.rows() {
            lines.push(screen.row_text(row));
            for (start, end, style) in screen.row_style_spans(row) {
                spans.push(Span {
                    row,
                    start,
                    end,
                    style,
                });
            }
        }
        Frame {
            cols: screen.cols(),
            rows: screen.rows(),
            lines,
            spans,
        }
    }

    /// Number of rows with any visible content, i.e. the row count after
    /// trimming trailing blank rows.
    pub fn used_rows(&self) -> usize {
        let mut used = self.rows;
        while used > 0
            && self.lines[used - 1].is_empty()
            && !self.spans.iter().any(|s| s.row == used - 1)
        {
            used -= 1;
        }
        used
    }
}

/// A complete recorded snapshot: the command, its rendered screen, its
/// stripped stderr and its exit code — the whole observable contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub cmd: Vec<String>,
    pub exit: i32,
    pub frame: Frame,
    pub stderr: Vec<String>,
}

/// A parse failure, positioned at a 1-based line of the `.snap` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapError {
    pub line: usize,
    pub msg: String,
}

impl fmt::Display for SnapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}

impl std::error::Error for SnapError {}

const MAGIC: &str = "ansisnap snapshot v1";

impl Snapshot {
    /// Serialize to the on-disk text form.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str(MAGIC);
        out.push('\n');
        out.push_str("cmd: ");
        out.push_str(&encode_cmd(&self.cmd));
        out.push('\n');
        out.push_str(&format!("term: {}x{}\n", self.frame.cols, self.frame.rows));
        out.push_str(&format!("exit: {}\n", self.exit));
        out.push_str(&format!(
            "--- screen: {} rows x {} cols ---\n",
            self.frame.rows, self.frame.cols
        ));
        for line in &self.frame.lines {
            out.push('|');
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(&format!(
            "--- styles: {} {} ---\n",
            self.frame.spans.len(),
            plural(self.frame.spans.len(), "span", "spans")
        ));
        for s in &self.frame.spans {
            out.push_str(&format!("r{} c{}-c{} {}\n", s.row, s.start, s.end, s.style));
        }
        out.push_str(&format!(
            "--- stderr: {} {} ---\n",
            self.stderr.len(),
            plural(self.stderr.len(), "line", "lines")
        ));
        for line in &self.stderr {
            out.push('|');
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    /// Parse the on-disk text form back.
    pub fn parse(text: &str) -> Result<Snapshot, SnapError> {
        let mut lines = text.lines().enumerate().map(|(i, l)| (i + 1, l));
        let err = |line: usize, msg: &str| SnapError {
            line,
            msg: msg.to_string(),
        };

        let (n, magic) = lines.next().ok_or_else(|| err(1, "empty file"))?;
        if magic != MAGIC {
            return Err(err(n, "not an ansisnap v1 snapshot"));
        }

        let (n, cmd_line) = lines.next().ok_or_else(|| err(2, "missing cmd header"))?;
        let cmd_raw = cmd_line
            .strip_prefix("cmd: ")
            .ok_or_else(|| err(n, "expected `cmd: [...]`"))?;
        let cmd = decode_cmd(cmd_raw).map_err(|msg| err(n, &msg))?;

        let (n, term_line) = lines.next().ok_or_else(|| err(3, "missing term header"))?;
        let dims = term_line
            .strip_prefix("term: ")
            .ok_or_else(|| err(n, "expected `term: <cols>x<rows>`"))?;
        let (cols, rows) = parse_dims(dims).ok_or_else(|| err(n, "bad terminal dimensions"))?;

        let (n, exit_line) = lines.next().ok_or_else(|| err(4, "missing exit header"))?;
        let exit: i32 = exit_line
            .strip_prefix("exit: ")
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| err(n, "expected `exit: <code>`"))?;

        let (n, screen_hdr) = lines
            .next()
            .ok_or_else(|| err(5, "missing screen section"))?;
        if !screen_hdr.starts_with("--- screen:") {
            return Err(err(n, "expected `--- screen: ... ---`"));
        }
        let mut frame_lines = Vec::with_capacity(rows);
        for _ in 0..rows {
            let (n, row) = lines
                .next()
                .ok_or_else(|| err(n, "screen section shorter than declared rows"))?;
            let content = row
                .strip_prefix('|')
                .ok_or_else(|| err(n, "screen row must start with `|`"))?;
            if visible_width(content) > cols {
                return Err(err(n, "screen row wider than declared cols"));
            }
            frame_lines.push(content.to_string());
        }

        let (n, styles_hdr) = lines
            .next()
            .ok_or_else(|| err(n, "missing styles section"))?;
        let span_count = section_count(styles_hdr, "--- styles: ", "span")
            .ok_or_else(|| err(n, "expected `--- styles: N spans ---`"))?;
        let mut spans = Vec::with_capacity(span_count);
        for _ in 0..span_count {
            let (n, line) = lines
                .next()
                .ok_or_else(|| err(n, "styles section shorter than declared count"))?;
            spans.push(parse_span(line, rows, cols).map_err(|msg| err(n, &msg))?);
        }

        let (n, stderr_hdr) = lines
            .next()
            .ok_or_else(|| err(n, "missing stderr section"))?;
        let stderr_count = section_count(stderr_hdr, "--- stderr: ", "line")
            .ok_or_else(|| err(n, "expected `--- stderr: N lines ---`"))?;
        let mut stderr = Vec::with_capacity(stderr_count);
        for _ in 0..stderr_count {
            let (n, line) = lines
                .next()
                .ok_or_else(|| err(n, "stderr section shorter than declared count"))?;
            let content = line
                .strip_prefix('|')
                .ok_or_else(|| err(n, "stderr line must start with `|`"))?;
            stderr.push(content.to_string());
        }

        if let Some((n, extra)) = lines.next() {
            if !extra.is_empty() {
                return Err(err(n, "trailing content after stderr section"));
            }
        }

        Ok(Snapshot {
            cmd,
            exit,
            frame: Frame {
                cols,
                rows,
                lines: frame_lines,
                spans,
            },
            stderr,
        })
    }
}

/// Sum of display widths — row width checks must count a CJK char as 2 cells.
fn visible_width(s: &str) -> usize {
    s.chars().map(|c| char_width(c) as usize).sum()
}

fn parse_dims(s: &str) -> Option<(usize, usize)> {
    let (c, r) = s.split_once('x')?;
    let cols: usize = c.parse().ok()?;
    let rows: usize = r.parse().ok()?;
    if (1..=1000).contains(&cols) && (1..=1000).contains(&rows) {
        Some((cols, rows))
    } else {
        None
    }
}

/// Pick the grammatically correct noun for a count — `1 span`, `2 spans`.
pub(crate) fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 {
        one
    } else {
        many
    }
}

/// Parse `<prefix>N <noun>[s] ---`. Both the singular and the plural noun
/// are accepted, so files written before the writer pluralized correctly
/// (`1 spans`) keep parsing.
fn section_count(line: &str, prefix: &str, noun: &str) -> Option<usize> {
    let rest = line.strip_prefix(prefix)?.strip_suffix(" ---")?;
    let (count, word) = rest.split_once(' ')?;
    if word != noun && word != format!("{noun}s") {
        return None;
    }
    count.parse().ok()
}

fn parse_span(line: &str, rows: usize, cols: usize) -> Result<Span, String> {
    let bad = || format!("bad style span `{line}`");
    let rest = line.strip_prefix('r').ok_or_else(bad)?;
    let (row, rest) = rest.split_once(" c").ok_or_else(bad)?;
    let (start, rest) = rest.split_once("-c").ok_or_else(bad)?;
    let (end, style) = rest.split_once(' ').ok_or_else(bad)?;
    let row: usize = row.parse().map_err(|_| bad())?;
    let start: usize = start.parse().map_err(|_| bad())?;
    let end: usize = end.parse().map_err(|_| bad())?;
    if row >= rows || end >= cols || start > end {
        return Err(format!("style span out of bounds `{line}`"));
    }
    let style = Style::parse(style).ok_or_else(|| format!("unknown style `{style}`"))?;
    Ok(Span {
        row,
        start,
        end,
        style,
    })
}

/// Encode a command vector as a JSON-style string array (a strict subset of
/// JSON: only `\\`, `\"`, `\n`, `\t` and `\u00XX` escapes are emitted).
pub fn encode_cmd(cmd: &[String]) -> String {
    let mut out = String::from("[");
    for (i, arg) in cmd.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('"');
        for c in arg.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
    }
    out.push(']');
    out
}

/// Decode the array form produced by [`encode_cmd`].
pub fn decode_cmd(s: &str) -> Result<Vec<String>, String> {
    let inner = s
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or("cmd must be a [\"...\"] array")?;
    let mut args = Vec::new();
    let mut chars = inner.chars().peekable();
    loop {
        match chars.next() {
            None => break,
            Some(',') if !args.is_empty() => {}
            Some('"') if args.is_empty() => {
                args.push(decode_string(&mut chars)?);
                continue;
            }
            Some(c) if args.is_empty() => return Err(format!("unexpected `{c}` in cmd array")),
            Some(c) => return Err(format!("expected `,` between cmd args, got `{c}`")),
        }
        match chars.next() {
            Some('"') => args.push(decode_string(&mut chars)?),
            other => return Err(format!("expected string after `,`, got {other:?}")),
        }
    }
    if args.is_empty() {
        return Err("cmd array is empty".to_string());
    }
    Ok(args)
}

fn decode_string(chars: &mut std::iter::Peekable<std::str::Chars>) -> Result<String, String> {
    let mut out = String::new();
    loop {
        match chars.next() {
            None => return Err("unterminated string in cmd array".to_string()),
            Some('"') => return Ok(out),
            Some('\\') => match chars.next() {
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    let v = u32::from_str_radix(&hex, 16)
                        .map_err(|_| format!("bad \\u escape `{hex}`"))?;
                    out.push(char::from_u32(v).ok_or("bad \\u code point")?);
                }
                other => return Err(format!("bad escape {other:?}")),
            },
            Some(c) => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{attr, Color};

    fn sample() -> Snapshot {
        let mut screen = Screen::new(20, 4);
        screen.feed_bytes(b"\x1b[1;32mPASS\x1b[0m all good\r\ntail  ");
        Snapshot {
            cmd: vec!["sh".into(), "-c".into(), "echo \"q\"\ttab".into()],
            exit: 0,
            frame: Frame::from_screen(&screen),
            stderr: vec!["warn: thing".into()],
        }
    }

    #[test]
    fn roundtrip_preserves_everything() {
        let mut snap = sample();
        snap.exit = 3;
        let text = snap.to_text();
        let parsed = Snapshot::parse(&text).unwrap();
        assert_eq!(parsed, snap);
    }

    #[test]
    fn serialized_form_is_reviewable() {
        let text = sample().to_text();
        assert!(text.starts_with("ansisnap snapshot v1\n"));
        assert!(text.contains("term: 20x4\n"));
        assert!(text.contains("|PASS all good\n"));
        assert!(text.contains("r0 c0-c3 bold,fg=green\n"));
        assert!(text.contains("--- stderr: 1 line ---\n|warn: thing\n"));
    }

    #[test]
    fn section_counts_pluralize_and_parse_accepts_both_forms() {
        // One span, one stderr line: the writer must not say `1 spans`.
        let text = sample().to_text();
        assert!(text.contains("--- styles: 1 span ---\n"));
        // Files written with the plural noun regardless of count (the
        // pre-polish form) must keep parsing — same v1 format.
        let legacy = text
            .replace("--- styles: 1 span ---", "--- styles: 1 spans ---")
            .replace("--- stderr: 1 line ---", "--- stderr: 1 lines ---");
        assert_eq!(Snapshot::parse(&legacy).unwrap(), sample());
        // A wrong noun is still a positioned parse error, not a zero count.
        let bad = text.replace("--- styles: 1 span ---", "--- styles: 1 spam ---");
        assert!(Snapshot::parse(&bad).is_err());
    }

    #[test]
    fn trailing_spaces_are_trimmed_from_rows() {
        let text = sample().to_text();
        assert!(text.contains("\n|tail\n"), "no trailing blanks on rows");
    }

    #[test]
    fn cmd_encoding_escapes_quotes_tabs_and_controls() {
        let cmd = vec!["a\"b".into(), "c\\d".into(), "e\nf".into(), "\u{1}".into()];
        let enc = encode_cmd(&cmd);
        assert_eq!(enc, r#"["a\"b","c\\d","e\nf","\u0001"]"#);
        assert_eq!(decode_cmd(&enc).unwrap(), cmd);
    }

    #[test]
    fn decode_cmd_rejects_malformed_input() {
        assert!(decode_cmd("not an array").is_err());
        assert!(decode_cmd("[]").is_err());
        assert!(decode_cmd(r#"["a" "b"]"#).is_err());
        assert!(decode_cmd(r#"["unterminated]"#).is_err());
        assert!(decode_cmd(r#"["bad\q"]"#).is_err());
    }

    #[test]
    fn parse_error_is_positioned() {
        let mut text = sample().to_text();
        // Corrupt the styles header (line 10: 4 headers + 1 + 4 rows).
        text = text.replace("--- styles:", "--- stylez:");
        let e = Snapshot::parse(&text).unwrap_err();
        assert_eq!(e.line, 10);
        assert!(e.to_string().contains("styles"));
    }

    #[test]
    fn wrong_magic_and_missing_row_prefix_are_rejected() {
        let e = Snapshot::parse("other snapshot v1\n").unwrap_err();
        assert_eq!(e.line, 1);
        let text = sample().to_text().replace("|PASS", "PASS");
        let e = Snapshot::parse(&text).unwrap_err();
        assert!(e.to_string().contains("start with `|`"));
    }

    #[test]
    fn too_wide_row_is_rejected_counting_wide_chars_double() {
        let mut snap = sample();
        snap.frame.lines[0] = "x".repeat(21); // cols is 20
        let e = Snapshot::parse(&snap.to_text()).unwrap_err();
        assert!(e.to_string().contains("wider than declared"));
        snap.frame.lines[0] = "字".repeat(11); // 22 cells in 20 cols
        assert!(Snapshot::parse(&snap.to_text()).is_err());
        snap.frame.lines[0] = "字".repeat(10); // exactly 20 cells
        assert!(Snapshot::parse(&snap.to_text()).is_ok());
    }

    #[test]
    fn out_of_bounds_span_is_rejected() {
        let mut snap = sample();
        snap.frame.spans[0].end = 25;
        let e = Snapshot::parse(&snap.to_text()).unwrap_err();
        assert!(e.to_string().contains("out of bounds"));
    }

    #[test]
    fn used_rows_ignores_trailing_blank_rows() {
        let snap = sample();
        assert_eq!(snap.frame.rows, 4);
        assert_eq!(snap.frame.used_rows(), 2);
    }

    #[test]
    fn styled_blank_row_counts_as_used() {
        // A row that is all spaces but painted with a background is content.
        let mut screen = Screen::new(10, 3);
        screen.feed_bytes(b"x\x1b[3;1H\x1b[44m   \x1b[0m");
        let frame = Frame::from_screen(&screen);
        assert_eq!(frame.lines[2], "");
        assert_eq!(frame.used_rows(), 3);
        assert_eq!(frame.spans.last().unwrap().style.bg, Color::Named(4));
        let _ = attr::BOLD; // silence unused import in cfg(test)
    }
}
