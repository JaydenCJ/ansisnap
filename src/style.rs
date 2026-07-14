//! Cell styles and SGR (Select Graphic Rendition) handling.
//!
//! A [`Style`] is what a cell *looks like*: attribute flags plus foreground
//! and background color. Styles serialize to a short, stable, human-readable
//! form (`bold,fg=red`) that appears verbatim in snapshot files and diffs, so
//! a style regression reads like English instead of `\x1b[1;31m`.

use crate::parser::Param;
use std::fmt;

/// Terminal color as the application requested it — we never resolve palette
/// indices to RGB, because snapshots must not depend on anyone's theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Color {
    #[default]
    Default,
    /// The classic 16 colors: 0–7 normal, 8–15 bright.
    Named(u8),
    /// 256-color palette index (16–255; 0–15 normalize to `Named`).
    Indexed(u8),
    /// 24-bit truecolor.
    Rgb(u8, u8, u8),
}

const NAMES: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Color::Default => write!(f, "default"),
            Color::Named(n) if n < 8 => write!(f, "{}", NAMES[n as usize]),
            Color::Named(n) => write!(f, "bright-{}", NAMES[(n % 8) as usize]),
            Color::Indexed(i) => write!(f, "idx{i}"),
            Color::Rgb(r, g, b) => write!(f, "#{r:02x}{g:02x}{b:02x}"),
        }
    }
}

impl Color {
    /// Parse the display form back. Used by the snapshot reader.
    pub fn parse(s: &str) -> Option<Color> {
        if s == "default" {
            return Some(Color::Default);
        }
        if let Some(hex) = s.strip_prefix('#') {
            if hex.len() == 6 {
                let v = u32::from_str_radix(hex, 16).ok()?;
                return Some(Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8));
            }
            return None;
        }
        if let Some(i) = s.strip_prefix("idx") {
            return i.parse::<u8>().ok().map(Color::normalize_index);
        }
        if let Some(name) = s.strip_prefix("bright-") {
            let n = NAMES.iter().position(|x| *x == name)?;
            return Some(Color::Named(n as u8 + 8));
        }
        let n = NAMES.iter().position(|x| *x == s)?;
        Some(Color::Named(n as u8))
    }

    fn normalize_index(i: u8) -> Color {
        if i < 16 {
            Color::Named(i)
        } else {
            Color::Indexed(i)
        }
    }
}

/// Attribute bit flags.
pub mod attr {
    pub const BOLD: u16 = 1 << 0;
    pub const DIM: u16 = 1 << 1;
    pub const ITALIC: u16 = 1 << 2;
    pub const UNDERLINE: u16 = 1 << 3;
    pub const BLINK: u16 = 1 << 4;
    pub const REVERSE: u16 = 1 << 5;
    pub const HIDDEN: u16 = 1 << 6;
    pub const STRIKE: u16 = 1 << 7;
}

/// Ordered (flag, name) table shared by Display and parse.
const ATTR_NAMES: [(u16, &str); 8] = [
    (attr::BOLD, "bold"),
    (attr::DIM, "dim"),
    (attr::ITALIC, "italic"),
    (attr::UNDERLINE, "underline"),
    (attr::BLINK, "blink"),
    (attr::REVERSE, "reverse"),
    (attr::HIDDEN, "hidden"),
    (attr::STRIKE, "strike"),
];

/// What one cell looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Style {
    pub attrs: u16,
    pub fg: Color,
    pub bg: Color,
}

impl Style {
    pub fn is_default(&self) -> bool {
        *self == Style::default()
    }

    fn set(&mut self, flag: u16, on: bool) {
        if on {
            self.attrs |= flag;
        } else {
            self.attrs &= !flag;
        }
    }

    /// Apply one SGR parameter list (the payload of a `CSI ... m`) to this
    /// style, mutating it the way a terminal's "current pen" mutates.
    pub fn apply_sgr(&mut self, params: &[Param]) {
        if params.is_empty() {
            *self = Style::default();
            return;
        }
        let mut i = 0;
        while i < params.len() {
            let p = &params[i];
            match p.value() {
                0 => *self = Style::default(),
                1 => self.set(attr::BOLD, true),
                2 => self.set(attr::DIM, true),
                3 => self.set(attr::ITALIC, true),
                4 => self.set(attr::UNDERLINE, true),
                5 | 6 => self.set(attr::BLINK, true),
                7 => self.set(attr::REVERSE, true),
                8 => self.set(attr::HIDDEN, true),
                9 => self.set(attr::STRIKE, true),
                21 | 22 => {
                    self.set(attr::BOLD, false);
                    self.set(attr::DIM, false);
                }
                23 => self.set(attr::ITALIC, false),
                24 => self.set(attr::UNDERLINE, false),
                25 => self.set(attr::BLINK, false),
                27 => self.set(attr::REVERSE, false),
                28 => self.set(attr::HIDDEN, false),
                29 => self.set(attr::STRIKE, false),
                30..=37 => self.fg = Color::Named((p.value() - 30) as u8),
                38 => i += self.extended_color(params, i, true),
                39 => self.fg = Color::Default,
                40..=47 => self.bg = Color::Named((p.value() - 40) as u8),
                48 => i += self.extended_color(params, i, false),
                49 => self.bg = Color::Default,
                90..=97 => self.fg = Color::Named((p.value() - 90 + 8) as u8),
                100..=107 => self.bg = Color::Named((p.value() - 100 + 8) as u8),
                _ => {} // unknown SGR: ignore, never corrupt the pen
            }
            i += 1;
        }
    }

    /// Handle `38`/`48` extended colors in both the colon form
    /// (`38:5:196` — one grouped param) and the semicolon form
    /// (`38;5;196` — consumes following params). Returns how many *extra*
    /// params were consumed.
    fn extended_color(&mut self, params: &[Param], i: usize, is_fg: bool) -> usize {
        let sub = &params[i].0;
        let (color, consumed) = if sub.len() > 1 {
            // ITU T.416 colon form may carry a colorspace-id slot:
            // `38:2:<colorspace>:r:g:b` (xterm/kitty accept it, usually
            // empty). Only the grouped form is unambiguous enough to skip
            // it — in the `;` form a sixth param is a separate SGR code.
            if sub[1] == 2 && sub.len() >= 6 {
                let rgb = [2, sub[3], sub[4], sub[5]];
                (Self::decode_extended(&rgb), 0)
            } else {
                (Self::decode_extended(&sub[1..]), 0)
            }
        } else {
            let rest: Vec<u16> = params[i + 1..].iter().map(Param::value).collect();
            let color = Self::decode_extended(&rest);
            let used = match rest.first() {
                Some(5) => 2.min(rest.len()),
                Some(2) => 4.min(rest.len()),
                _ => 0,
            };
            (color, used)
        };
        if let Some(c) = color {
            if is_fg {
                self.fg = c;
            } else {
                self.bg = c;
            }
        }
        consumed
    }

    fn decode_extended(v: &[u16]) -> Option<Color> {
        match v.first()? {
            5 => {
                let idx = *v.get(1)? as u8;
                Some(Color::normalize_index(idx))
            }
            2 => {
                let (r, g, b) = (*v.get(1)?, *v.get(2)?, *v.get(3)?);
                Some(Color::Rgb(r as u8, g as u8, b as u8))
            }
            _ => None,
        }
    }

    /// Parse the display form (`bold,underline,fg=red,bg=idx17`).
    pub fn parse(s: &str) -> Option<Style> {
        let mut style = Style::default();
        for part in s.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some(c) = part.strip_prefix("fg=") {
                style.fg = Color::parse(c)?;
            } else if let Some(c) = part.strip_prefix("bg=") {
                style.bg = Color::parse(c)?;
            } else {
                let (flag, _) = ATTR_NAMES.iter().find(|(_, n)| *n == part)?;
                style.attrs |= flag;
            }
        }
        Some(style)
    }
}

impl fmt::Display for Style {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        let mut sep = |f: &mut fmt::Formatter<'_>| -> fmt::Result {
            if !first {
                write!(f, ",")?;
            }
            first = false;
            Ok(())
        };
        for (flag, name) in ATTR_NAMES {
            if self.attrs & flag != 0 {
                sep(f)?;
                write!(f, "{name}")?;
            }
        }
        if self.fg != Color::Default {
            sep(f)?;
            write!(f, "fg={}", self.fg)?;
        }
        if self.bg != Color::Default {
            sep(f)?;
            write!(f, "bg={}", self.bg)?;
        }
        if first {
            write!(f, "plain")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sgr(style: &mut Style, params: &[&[u16]]) {
        let p: Vec<Param> = params.iter().map(|v| Param(v.to_vec())).collect();
        style.apply_sgr(&p);
    }

    #[test]
    fn basic_attributes_set_clear_and_empty_sgr_resets() {
        let mut s = Style::default();
        sgr(&mut s, &[&[1], &[4]]);
        assert_eq!(s.attrs, attr::BOLD | attr::UNDERLINE);
        sgr(&mut s, &[&[22], &[24]]);
        assert!(s.is_default());
        // Bare `ESC [ m` (no parameters) is a full reset.
        let mut s = Style {
            attrs: attr::BOLD,
            fg: Color::Named(1),
            bg: Color::Default,
        };
        s.apply_sgr(&[]);
        assert!(s.is_default());
    }

    #[test]
    fn named_and_bright_colors() {
        let mut s = Style::default();
        sgr(&mut s, &[&[31], &[42]]);
        assert_eq!(s.fg, Color::Named(1));
        assert_eq!(s.bg, Color::Named(2));
        sgr(&mut s, &[&[91], &[102]]);
        assert_eq!(s.fg, Color::Named(9));
        assert_eq!(s.bg, Color::Named(10));
    }

    #[test]
    fn indexed_color_semicolon_form_consumes_following_params() {
        // `38;5;196;1m` — the `1` after the color is bold, not part of it.
        let mut s = Style::default();
        sgr(&mut s, &[&[38], &[5], &[196], &[1]]);
        assert_eq!(s.fg, Color::Indexed(196));
        assert_ne!(s.attrs & attr::BOLD, 0);
    }

    #[test]
    fn truecolor_semicolon_and_colon_forms() {
        let mut s = Style::default();
        sgr(&mut s, &[&[48], &[2], &[10], &[20], &[30]]);
        assert_eq!(s.bg, Color::Rgb(10, 20, 30));
        sgr(&mut s, &[&[38, 2, 255, 128, 0]]);
        assert_eq!(s.fg, Color::Rgb(255, 128, 0));
        // ITU T.416 form `38:2::255:100:0` — the empty colorspace-id must
        // not be consumed as the red component (xterm and kitty skip it).
        sgr(&mut s, &[&[38, 2, 0, 255, 100, 0]]);
        assert_eq!(s.fg, Color::Rgb(255, 100, 0));
    }

    #[test]
    fn low_palette_indices_normalize_to_named() {
        // `38;5;1` and `31` must compare equal — same visible color.
        let mut a = Style::default();
        let mut b = Style::default();
        sgr(&mut a, &[&[38], &[5], &[1]]);
        sgr(&mut b, &[&[31]]);
        assert_eq!(a, b);
    }

    #[test]
    fn truncated_and_unknown_sgr_codes_are_ignored() {
        let mut s = Style::default();
        sgr(&mut s, &[&[38], &[2], &[10]]); // missing g, b
        assert_eq!(s.fg, Color::Default);
        sgr(&mut s, &[&[73], &[31]]); // 73 is not an SGR we know
        assert_eq!(s.fg, Color::Named(1));
        assert_eq!(s.attrs & attr::BOLD, 0);
    }

    #[test]
    fn display_is_stable_ordered_and_roundtrips() {
        let s = Style {
            attrs: attr::UNDERLINE | attr::BOLD,
            fg: Color::Named(9),
            bg: Color::Indexed(17),
        };
        assert_eq!(s.to_string(), "bold,underline,fg=bright-red,bg=idx17");
        assert_eq!(Style::default().to_string(), "plain");
        let cases = [
            Style::default(),
            Style {
                attrs: attr::BOLD | attr::STRIKE,
                fg: Color::Rgb(1, 2, 3),
                bg: Color::Named(0),
            },
            Style {
                attrs: attr::REVERSE,
                fg: Color::Indexed(200),
                bg: Color::Named(15),
            },
        ];
        for s in cases {
            let text = s.to_string();
            let parsed = if text == "plain" {
                Style::default()
            } else {
                Style::parse(&text).unwrap()
            };
            assert_eq!(parsed, s, "roundtrip failed for {text}");
        }
    }

    #[test]
    fn color_parse_rejects_garbage() {
        assert_eq!(Color::parse("chartreuse"), None);
        assert_eq!(Color::parse("#12345"), None);
        assert_eq!(Color::parse("idx999"), None);
    }
}
