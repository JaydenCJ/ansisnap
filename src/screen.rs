//! The terminal emulator: a fixed-size cell grid that applies parsed actions.
//!
//! This is the piece that makes ansisnap different from byte-level snapshot
//! tools: `\r`-overdrawn progress bars, cursor-addressed TUI frames and
//! erase-line repaints all collapse into the *final visible screen*, which is
//! what a human would actually see and what a test should assert on.
//!
//! Implemented VT/xterm subset: cursor movement (CUP/CUU/CUD/CUF/CUB/CNL/CPL/
//! CHA/VPA), erases (ED/EL/ECH), edits (ICH/DCH/IL/DL), scrolling (IND/RI/NEL/
//! SU/SD, DECSTBM scroll regions), SGR styling, autowrap with deferred wrap,
//! tab stops, save/restore cursor, and the alternate screen buffer.

use crate::parser::{Action, Param, Parser};
use crate::style::Style;

/// One grid cell. `width == 0` marks the continuation cell of a wide
/// character; `width == 2` marks its head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
    pub width: u8,
}

impl Cell {
    pub fn blank() -> Cell {
        Cell {
            ch: ' ',
            style: Style::default(),
            width: 1,
        }
    }

    pub fn is_blank(&self) -> bool {
        self.ch == ' ' && self.style.is_default()
    }
}

/// Display width of a character on a terminal: 0 (combining/zero-width),
/// 1, or 2 (East Asian wide). A compact range table covering the ranges that
/// actually show up in CLI output (CJK, Hangul, kana, fullwidth forms, emoji).
pub fn char_width(c: char) -> u8 {
    let u = c as u32;
    // Zero width: combining marks, ZWJ/ZWSP, variation selectors.
    if matches!(u,
        0x0300..=0x036f | 0x1ab0..=0x1aff | 0x20d0..=0x20ff
        | 0x200b..=0x200f | 0xfe00..=0xfe0f | 0xfe20..=0xfe2f | 0xfeff)
    {
        return 0;
    }
    if matches!(u,
        0x1100..=0x115f            // Hangul Jamo
        | 0x2e80..=0x303e          // CJK radicals, punctuation
        | 0x3041..=0x33ff          // kana, CJK symbols
        | 0x3400..=0x4dbf          // CJK ext A
        | 0x4e00..=0x9fff          // CJK unified
        | 0xa000..=0xa4cf          // Yi
        | 0xac00..=0xd7a3          // Hangul syllables
        | 0xf900..=0xfaff          // CJK compat
        | 0xfe30..=0xfe4f          // CJK compat forms
        | 0xff00..=0xff60          // fullwidth forms
        | 0xffe0..=0xffe6
        | 0x1f300..=0x1f64f        // emoji
        | 0x1f680..=0x1f6ff
        | 0x1f900..=0x1f9ff
        | 0x20000..=0x2fffd | 0x30000..=0x3fffd)
    {
        return 2;
    }
    1
}

#[derive(Debug, Clone)]
struct SavedCursor {
    row: usize,
    col: usize,
    pen: Style,
}

/// A fixed-size terminal screen.
pub struct Screen {
    cols: usize,
    rows: usize,
    grid: Vec<Cell>,
    cur_row: usize,
    cur_col: usize,
    pen: Style,
    /// Deferred wrap: after printing in the last column, real terminals park
    /// the cursor *on* that column and only wrap when the next glyph arrives.
    wrap_pending: bool,
    autowrap: bool,
    scroll_top: usize,
    scroll_bottom: usize, // inclusive
    saved: Option<SavedCursor>,
    alt: Option<AltState>,
}

struct AltState {
    grid: Vec<Cell>,
    cur_row: usize,
    cur_col: usize,
    pen: Style,
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Screen {
        let cols = cols.clamp(1, 1000);
        let rows = rows.clamp(1, 1000);
        Screen {
            cols,
            rows,
            grid: vec![Cell::blank(); cols * rows],
            cur_row: 0,
            cur_col: 0,
            pen: Style::default(),
            wrap_pending: false,
            autowrap: true,
            scroll_top: 0,
            scroll_bottom: rows - 1,
            saved: None,
            alt: None,
        }
    }

    /// Parse and apply a complete byte buffer.
    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        for action in Parser::parse(bytes) {
            self.apply(&action);
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.cur_row, self.cur_col)
    }

    pub fn cell(&self, row: usize, col: usize) -> &Cell {
        &self.grid[row * self.cols + col]
    }

    fn cell_mut(&mut self, row: usize, col: usize) -> &mut Cell {
        &mut self.grid[row * self.cols + col]
    }

    /// The visible text of one row, trailing blanks trimmed. Wide-char
    /// continuation cells contribute nothing (the head char covers them).
    pub fn row_text(&self, row: usize) -> String {
        let mut s = String::new();
        for col in 0..self.cols {
            let c = self.cell(row, col);
            if c.width != 0 {
                s.push(c.ch);
            }
        }
        s.truncate(s.trim_end_matches(' ').len());
        s
    }

    /// Contiguous non-default style runs of one row: `(start_col, end_col
    /// inclusive, style)`.
    pub fn row_style_spans(&self, row: usize) -> Vec<(usize, usize, Style)> {
        let mut spans = Vec::new();
        let mut cur: Option<(usize, usize, Style)> = None;
        for col in 0..self.cols {
            let style = self.cell(row, col).style;
            match cur {
                Some((start, _, s)) if s == style => cur = Some((start, col, s)),
                _ => {
                    if let Some(span) = cur.take() {
                        if !span.2.is_default() {
                            spans.push(span);
                        }
                    }
                    cur = Some((col, col, style));
                }
            }
        }
        if let Some(span) = cur {
            if !span.2.is_default() {
                spans.push(span);
            }
        }
        spans
    }

    /// Apply one decoded action.
    pub fn apply(&mut self, action: &Action) {
        match action {
            Action::Print(c) => self.print(*c),
            Action::Control(b) => self.control(*b),
            Action::Csi {
                private,
                params,
                intermediate,
                final_byte,
            } => self.csi(*private, params, *intermediate, *final_byte),
            Action::Esc {
                intermediate,
                final_byte,
            } => self.esc(*intermediate, *final_byte),
            Action::Osc(_) => {} // titles etc. never reach the grid
        }
    }

    // --- printing ------------------------------------------------------

    fn print(&mut self, c: char) {
        let w = char_width(c);
        if w == 0 {
            return; // combining marks: out of scope for grid comparison
        }
        let w = w as usize;
        if self.wrap_pending {
            if self.autowrap {
                self.wrap_pending = false;
                self.cur_col = 0;
                self.line_feed();
            } else {
                self.cur_col = self.cols - 1;
                self.wrap_pending = false;
            }
        }
        // A wide char that no longer fits on this line wraps early.
        if w == 2 && self.cur_col + 2 > self.cols {
            if self.autowrap {
                self.cur_col = 0;
                self.line_feed();
            } else {
                return; // cannot render half a glyph
            }
        }
        let (row, col) = (self.cur_row, self.cur_col);
        // Overwriting half of an existing wide char orphans the other half.
        self.clear_wide_at(row, col);
        if w == 2 {
            self.clear_wide_at(row, col + 1);
        }
        *self.cell_mut(row, col) = Cell {
            ch: c,
            style: self.pen,
            width: w as u8,
        };
        if w == 2 {
            *self.cell_mut(row, col + 1) = Cell {
                ch: ' ',
                style: self.pen,
                width: 0,
            };
        }
        if col + w >= self.cols {
            self.cur_col = self.cols - 1;
            self.wrap_pending = true;
        } else {
            self.cur_col = col + w;
        }
    }

    /// If (row, col) holds part of a wide char, blank both halves.
    fn clear_wide_at(&mut self, row: usize, col: usize) {
        if col >= self.cols {
            return;
        }
        let cell = *self.cell(row, col);
        if cell.width == 2 && col + 1 < self.cols {
            *self.cell_mut(row, col + 1) = Cell::blank();
        } else if cell.width == 0 && col > 0 {
            *self.cell_mut(row, col - 1) = Cell::blank();
        }
    }

    // --- control bytes ---------------------------------------------------

    fn control(&mut self, b: u8) {
        match b {
            // LF/VT/FF: line feed *with* carriage return. ansisnap captures
            // through a pipe, where the tty line discipline (ONLCR) that
            // would normally add the CR is absent — the emulator supplies it,
            // so the grid matches what a terminal would actually display.
            b'\n' | 0x0b | 0x0c => {
                self.line_feed();
                self.cur_col = 0;
            }
            b'\r' => {
                self.cur_col = 0;
                self.wrap_pending = false;
            }
            0x08 => {
                // BS moves, never erases.
                self.cur_col = self.cur_col.saturating_sub(1);
                self.wrap_pending = false;
            }
            b'\t' => {
                let next = (self.cur_col / 8 + 1) * 8;
                self.cur_col = next.min(self.cols - 1);
                self.wrap_pending = false;
            }
            _ => {}
        }
    }

    fn line_feed(&mut self) {
        if self.cur_row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cur_row + 1 < self.rows {
            self.cur_row += 1;
        }
        self.wrap_pending = false;
    }

    fn reverse_line_feed(&mut self) {
        if self.cur_row == self.scroll_top {
            self.scroll_down(1);
        } else {
            self.cur_row = self.cur_row.saturating_sub(1);
        }
        self.wrap_pending = false;
    }

    fn scroll_up(&mut self, n: usize) {
        let n = n.min(self.scroll_bottom - self.scroll_top + 1);
        for _ in 0..n {
            for row in self.scroll_top..self.scroll_bottom {
                for col in 0..self.cols {
                    *self.cell_mut(row, col) = *self.cell(row + 1, col);
                }
            }
            self.blank_row(self.scroll_bottom);
        }
    }

    fn scroll_down(&mut self, n: usize) {
        let n = n.min(self.scroll_bottom - self.scroll_top + 1);
        for _ in 0..n {
            for row in (self.scroll_top + 1..=self.scroll_bottom).rev() {
                for col in 0..self.cols {
                    *self.cell_mut(row, col) = *self.cell(row - 1, col);
                }
            }
            self.blank_row(self.scroll_top);
        }
    }

    fn blank_row(&mut self, row: usize) {
        for col in 0..self.cols {
            *self.cell_mut(row, col) = Cell::blank();
        }
    }

    // --- escape dispatch --------------------------------------------------

    fn esc(&mut self, intermediate: Option<char>, final_byte: char) {
        if intermediate.is_some() {
            return; // charset designations: no visible effect here
        }
        match final_byte {
            'D' => self.line_feed(), // IND
            'E' => {
                self.line_feed(); // NEL
                self.cur_col = 0;
            }
            'M' => self.reverse_line_feed(), // RI
            '7' => self.save_cursor(),
            '8' => self.restore_cursor(),
            'c' => self.full_reset(), // RIS
            _ => {}
        }
    }

    fn full_reset(&mut self) {
        let (cols, rows) = (self.cols, self.rows);
        *self = Screen::new(cols, rows);
    }

    fn save_cursor(&mut self) {
        self.saved = Some(SavedCursor {
            row: self.cur_row,
            col: self.cur_col,
            pen: self.pen,
        });
    }

    fn restore_cursor(&mut self) {
        if let Some(s) = &self.saved {
            self.cur_row = s.row.min(self.rows - 1);
            self.cur_col = s.col.min(self.cols - 1);
            self.pen = s.pen;
        } else {
            self.cur_row = 0;
            self.cur_col = 0;
            self.pen = Style::default();
        }
        self.wrap_pending = false;
    }

    // --- CSI dispatch -----------------------------------------------------

    fn csi(
        &mut self,
        private: Option<char>,
        params: &[Param],
        intermediate: Option<char>,
        final_byte: char,
    ) {
        if intermediate.is_some() {
            return;
        }
        if private == Some('?') {
            self.dec_private(params, final_byte);
            return;
        }
        if private.is_some() {
            return;
        }
        let p = |i: usize| params.get(i).map(Param::value).unwrap_or(0) as usize;
        let n1 = |i: usize| p(i).max(1);
        self.wrap_pending = false;
        match final_byte {
            'A' => self.cur_row = self.cur_row.saturating_sub(n1(0)).max(self.top_bound()),
            'B' => self.cur_row = (self.cur_row + n1(0)).min(self.bottom_bound()),
            'C' => self.cur_col = (self.cur_col + n1(0)).min(self.cols - 1),
            'D' => self.cur_col = self.cur_col.saturating_sub(n1(0)),
            'E' => {
                self.cur_row = (self.cur_row + n1(0)).min(self.bottom_bound());
                self.cur_col = 0;
            }
            'F' => {
                self.cur_row = self.cur_row.saturating_sub(n1(0)).max(self.top_bound());
                self.cur_col = 0;
            }
            'G' => self.cur_col = (n1(0) - 1).min(self.cols - 1),
            'H' | 'f' => {
                self.cur_row = (n1(0) - 1).min(self.rows - 1);
                self.cur_col = (n1(1) - 1).min(self.cols - 1);
            }
            'd' => self.cur_row = (n1(0) - 1).min(self.rows - 1),
            'J' => self.erase_display(p(0)),
            'K' => self.erase_line(p(0)),
            'L' => self.insert_lines(n1(0)),
            'M' => self.delete_lines(n1(0)),
            'P' => self.delete_chars(n1(0)),
            '@' => self.insert_chars(n1(0)),
            'X' => self.erase_chars(n1(0)),
            'S' => self.scroll_up(n1(0)),
            'T' => self.scroll_down(n1(0)),
            'm' => self.pen.apply_sgr(params),
            'r' => self.set_scroll_region(p(0), p(1)),
            's' => self.save_cursor(),
            'u' => self.restore_cursor(),
            _ => {} // queries (n, c, t...) have no grid effect
        }
    }

    /// Cursor-up cannot leave the scroll region if it started inside it.
    fn top_bound(&self) -> usize {
        if self.cur_row >= self.scroll_top {
            self.scroll_top
        } else {
            0
        }
    }

    fn bottom_bound(&self) -> usize {
        if self.cur_row <= self.scroll_bottom {
            self.scroll_bottom
        } else {
            self.rows - 1
        }
    }

    fn dec_private(&mut self, params: &[Param], final_byte: char) {
        for p in params {
            match (p.value(), final_byte) {
                (7, 'h') => self.autowrap = true,
                (7, 'l') => self.autowrap = false,
                (47 | 1047 | 1049, 'h') => self.enter_alt(),
                (47 | 1047 | 1049, 'l') => self.leave_alt(),
                // 25 (cursor visibility), 2004 (bracketed paste), 1000-range
                // (mouse): stateful in a real terminal, invisible in a grid.
                _ => {}
            }
        }
    }

    fn enter_alt(&mut self) {
        if self.alt.is_some() {
            return; // already on the alternate screen
        }
        let blank = vec![Cell::blank(); self.cols * self.rows];
        self.alt = Some(AltState {
            grid: std::mem::replace(&mut self.grid, blank),
            cur_row: self.cur_row,
            cur_col: self.cur_col,
            pen: self.pen,
        });
        self.cur_row = 0;
        self.cur_col = 0;
        self.wrap_pending = false;
    }

    fn leave_alt(&mut self) {
        if let Some(main) = self.alt.take() {
            self.grid = main.grid;
            self.cur_row = main.cur_row;
            self.cur_col = main.cur_col;
            self.pen = main.pen;
            self.wrap_pending = false;
        }
    }

    // --- erases and edits ---------------------------------------------------

    fn erase_display(&mut self, mode: usize) {
        match mode {
            0 => {
                self.erase_line(0);
                for row in self.cur_row + 1..self.rows {
                    self.blank_row(row);
                }
            }
            1 => {
                self.erase_line(1);
                for row in 0..self.cur_row {
                    self.blank_row(row);
                }
            }
            2 | 3 => {
                for row in 0..self.rows {
                    self.blank_row(row);
                }
            }
            _ => {}
        }
    }

    fn erase_line(&mut self, mode: usize) {
        let (from, to) = match mode {
            0 => (self.cur_col, self.cols - 1),
            1 => (0, self.cur_col),
            2 => (0, self.cols - 1),
            _ => return,
        };
        let row = self.cur_row;
        self.clear_wide_at(row, from);
        self.clear_wide_at(row, to);
        for col in from..=to {
            *self.cell_mut(row, col) = Cell::blank();
        }
    }

    fn erase_chars(&mut self, n: usize) {
        let row = self.cur_row;
        let end = (self.cur_col + n).min(self.cols);
        self.clear_wide_at(row, self.cur_col);
        if end > 0 {
            self.clear_wide_at(row, end - 1);
        }
        for col in self.cur_col..end {
            *self.cell_mut(row, col) = Cell::blank();
        }
    }

    fn insert_lines(&mut self, n: usize) {
        if self.cur_row < self.scroll_top || self.cur_row > self.scroll_bottom {
            return;
        }
        let saved_top = self.scroll_top;
        self.scroll_top = self.cur_row;
        self.scroll_down(n);
        self.scroll_top = saved_top;
        self.cur_col = 0;
    }

    fn delete_lines(&mut self, n: usize) {
        if self.cur_row < self.scroll_top || self.cur_row > self.scroll_bottom {
            return;
        }
        let saved_top = self.scroll_top;
        self.scroll_top = self.cur_row;
        self.scroll_up(n);
        self.scroll_top = saved_top;
        self.cur_col = 0;
    }

    fn delete_chars(&mut self, n: usize) {
        let row = self.cur_row;
        let n = n.min(self.cols - self.cur_col);
        self.clear_wide_at(row, self.cur_col);
        for col in self.cur_col..self.cols {
            *self.cell_mut(row, col) = if col + n < self.cols {
                *self.cell(row, col + n)
            } else {
                Cell::blank()
            };
        }
    }

    fn insert_chars(&mut self, n: usize) {
        let row = self.cur_row;
        let n = n.min(self.cols - self.cur_col);
        self.clear_wide_at(row, self.cur_col);
        for col in (self.cur_col..self.cols).rev() {
            *self.cell_mut(row, col) = if col >= self.cur_col + n {
                *self.cell(row, col - n)
            } else {
                Cell::blank()
            };
        }
    }

    fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        let top = top.max(1) - 1;
        let bottom = if bottom == 0 { self.rows } else { bottom } - 1;
        if top < bottom && bottom < self.rows {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
            self.cur_row = 0;
            self.cur_col = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{attr, Color};

    fn screen(cols: usize, rows: usize, input: &[u8]) -> Screen {
        let mut s = Screen::new(cols, rows);
        s.feed_bytes(input);
        s
    }

    fn text(s: &Screen) -> Vec<String> {
        (0..s.rows()).map(|r| s.row_text(r)).collect()
    }

    #[test]
    fn plain_lines_land_on_rows() {
        let s = screen(10, 3, b"one\r\ntwo");
        assert_eq!(text(&s), vec!["one", "two", ""]);
    }

    #[test]
    fn bare_lf_implies_carriage_return_like_onlcr() {
        // Pipes bypass the tty ONLCR translation; the emulator supplies it,
        // so `echo`-style output lands where a terminal would show it.
        let s = screen(10, 3, b"ab\ncd");
        assert_eq!(text(&s), vec!["ab", "cd", ""]);
        // ESC D (IND) remains a pure line feed that keeps the column.
        let s = screen(10, 3, b"ab\x1bDcd");
        assert_eq!(text(&s), vec!["ab", "  cd", ""]);
    }

    #[test]
    fn carriage_return_overwrites_in_place() {
        // The progress-bar pattern: only the final state must survive.
        let s = screen(
            20,
            2,
            b"downloading 10%\rdownloading 99%\rdone.          \r\n",
        );
        assert_eq!(s.row_text(0), "done.");
    }

    #[test]
    fn autowrap_flows_to_next_line() {
        let s = screen(5, 3, b"abcdefg");
        assert_eq!(text(&s), vec!["abcde", "fg", ""]);
    }

    #[test]
    fn deferred_wrap_and_autowrap_off() {
        // Print exactly `cols` chars then CR: the cursor must still be on
        // row 0 (wrap is deferred until the next glyph).
        let mut s = screen(5, 3, b"abcde\rX");
        assert_eq!(s.row_text(0), "Xbcde");
        s.feed_bytes(b"\x1b[?7l");
        assert_eq!(s.cursor().0, 0);
        // With autowrap off every char past the edge overwrites column 5.
        let s = screen(5, 2, b"\x1b[?7labcdefgh");
        assert_eq!(text(&s), vec!["abcdh", ""]);
    }

    #[test]
    fn scroll_when_bottom_row_overflows() {
        let s = screen(10, 2, b"one\r\ntwo\r\nthree");
        assert_eq!(text(&s), vec!["two", "three"]);
    }

    #[test]
    fn cup_moves_and_clamps() {
        let mut s = screen(10, 5, b"\x1b[3;4Hx");
        assert_eq!(s.row_text(2), "   x");
        // Out-of-range coordinates clamp to the edges instead of panicking.
        s.feed_bytes(b"\x1b[99;99Hy");
        assert_eq!(s.cell(4, 9).ch, 'y');
    }

    #[test]
    fn relative_cursor_moves() {
        let s = screen(10, 5, b"\x1b[3;3Ho\x1b[A\x1b[2Du\x1b[B\x1b[Bd");
        assert_eq!(s.row_text(1), " u");
        assert_eq!(s.row_text(2), "  o");
        assert_eq!(s.row_text(3), "  d");
    }

    #[test]
    fn erase_line_variants() {
        let s = screen(10, 3, b"abcdefghij\x1b[1;5H\x1b[K");
        assert_eq!(s.row_text(0), "abcd");
        let s = screen(10, 3, b"abcdefghij\x1b[1;5H\x1b[1K");
        assert_eq!(s.row_text(0), "     fghij");
        let s = screen(10, 3, b"abcdefghij\x1b[2K");
        assert_eq!(s.row_text(0), "");
    }

    #[test]
    fn erase_display_below_and_above() {
        let s = screen(5, 3, b"aaaaa\r\nbbbbb\r\nccccc\x1b[2;3H\x1b[J");
        assert_eq!(text(&s), vec!["aaaaa", "bb", ""]);
        let s = screen(5, 3, b"aaaaa\r\nbbbbb\r\nccccc\x1b[2;3H\x1b[1J");
        assert_eq!(text(&s), vec!["", "   bb", "ccccc"]);
        // ED 2 clears everything but must leave the cursor in place.
        let mut s = screen(5, 3, b"aaaaa\r\nbbbbb\x1b[2J");
        assert_eq!(text(&s), vec!["", "", ""]);
        assert_eq!(s.cursor(), (1, 4)); // wrap-pending col clamps at edge
        s.feed_bytes(b"z");
        assert_eq!(s.row_text(1), "    z");
    }

    #[test]
    fn erase_chars_ech() {
        let s = screen(10, 1, b"abcdefghij\x1b[1;3H\x1b[4X");
        assert_eq!(s.row_text(0), "ab    ghij");
    }

    #[test]
    fn insert_and_delete_chars() {
        let s = screen(10, 1, b"abcdef\x1b[1;3H\x1b[2@");
        assert_eq!(s.row_text(0), "ab  cdef");
        let s = screen(10, 1, b"abcdef\x1b[1;3H\x1b[2P");
        assert_eq!(s.row_text(0), "abef");
    }

    #[test]
    fn insert_and_delete_lines() {
        let s = screen(5, 4, b"a\r\nb\r\nc\r\nd\x1b[2;1H\x1b[L");
        assert_eq!(text(&s), vec!["a", "", "b", "c"]);
        let s = screen(5, 4, b"a\r\nb\r\nc\r\nd\x1b[2;1H\x1b[M");
        assert_eq!(text(&s), vec!["a", "c", "d", ""]);
    }

    #[test]
    fn reverse_index_scrolls_at_top() {
        let s = screen(5, 3, b"a\r\nb\r\nc\x1b[1;1H\x1bMx");
        assert_eq!(text(&s), vec!["x", "a", "b"]);
    }

    #[test]
    fn scroll_region_confines_scrolling() {
        // Region rows 2-3 (1-based): scrolling inside must not touch row 1
        // or row 4 — the classic fixed-header TUI layout.
        let s = screen(6, 4, b"head\x1b[2;3r\x1b[2;1Hl1\r\nl2\r\nl3\r\nl4");
        assert_eq!(s.row_text(0), "head");
        assert_eq!(text(&s), vec!["head", "l3", "l4", ""]);
    }

    #[test]
    fn styled_print_records_pen() {
        let s = screen(10, 1, b"\x1b[1;31mhi\x1b[0m!");
        let bold_red = Style {
            attrs: attr::BOLD,
            fg: Color::Named(1),
            bg: Color::Default,
        };
        assert_eq!(s.cell(0, 0).style, bold_red);
        assert_eq!(s.cell(0, 1).style, bold_red);
        assert!(s.cell(0, 2).style.is_default());
    }

    #[test]
    fn style_spans_merge_adjacent_cells() {
        let s = screen(10, 1, b"\x1b[32mok\x1b[0m go \x1b[32mon\x1b[0m");
        let spans = s.row_style_spans(0);
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].0, spans[0].1), (0, 1));
        assert_eq!((spans[1].0, spans[1].1), (6, 7));
    }

    #[test]
    fn save_restore_cursor_and_pen() {
        let s = screen(10, 2, b"\x1b[31m\x1b7\x1b[2;5H\x1b[34mB\x1b8A");
        assert_eq!(s.cell(0, 0).ch, 'A');
        assert_eq!(s.cell(0, 0).style.fg, Color::Named(1)); // restored red pen
        assert_eq!(s.cell(1, 4).style.fg, Color::Named(4));
    }

    #[test]
    fn alternate_screen_shows_then_restores_main_content() {
        // TUI apps run on the alt screen; on exit the shell prompt returns.
        let s = screen(10, 2, b"main\x1b[?1049hTUI");
        assert_eq!(text(&s), vec!["TUI", ""]);
        let s = screen(10, 2, b"main\x1b[?1049htui-frame\x1b[?1049l");
        assert_eq!(text(&s), vec!["main", ""]);
        assert_eq!(s.cursor(), (0, 4));
    }

    #[test]
    fn full_reset_clears_everything() {
        let s = screen(6, 2, b"\x1b[31mhello\x1b[2;3r\x1bcx");
        assert_eq!(text(&s), vec!["x", ""]);
        assert!(s.cell(0, 0).style.is_default());
    }

    #[test]
    fn tabs_hit_eight_column_stops_and_backspace_never_erases() {
        let s = screen(20, 1, b"a\tb\tc");
        assert_eq!(s.row_text(0), "a       b       c");
        let s = screen(10, 1, b"ab\x08X");
        assert_eq!(s.row_text(0), "aX");
    }

    #[test]
    fn wide_char_occupies_two_cells() {
        let s = screen(10, 1, "日本".as_bytes());
        assert_eq!(s.row_text(0), "日本");
        assert_eq!(s.cell(0, 0).width, 2);
        assert_eq!(s.cell(0, 1).width, 0);
        assert_eq!(s.cursor(), (0, 4));
    }

    #[test]
    fn wide_char_at_last_column_wraps_early() {
        // Column 5 of 6 cannot hold a 2-cell glyph: it wraps whole.
        let s = screen(6, 2, "abcde字".as_bytes());
        assert_eq!(text(&s), vec!["abcde", "字"]);
    }

    #[test]
    fn overwriting_half_a_wide_char_blanks_the_other_half() {
        let s = screen(10, 1, "字\x1b[1;1HX".as_bytes());
        assert_eq!(s.row_text(0), "X");
        assert_eq!(s.cell(0, 1).ch, ' ');
        assert_eq!(s.cell(0, 1).width, 1);
    }

    #[test]
    fn cursor_up_stops_at_scroll_region_top() {
        let s = screen(6, 4, b"\x1b[2;3r\x1b[3;1H\x1b[9Ax");
        assert_eq!(s.cursor().0, 1); // row 2 (1-based) is the region top
        assert_eq!(s.row_text(1), "x");
    }

    #[test]
    fn combining_marks_are_dropped_not_printed() {
        let s = screen(10, 1, "e\u{0301}!".as_bytes());
        assert_eq!(s.row_text(0), "e!");
    }
}
