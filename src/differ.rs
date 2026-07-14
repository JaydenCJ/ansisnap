//! Grid-level comparison and human-readable failure reports.
//!
//! The differ works on [`Frame`]s — rendered screens — so a failure says
//! "row 4 says `wurld`, expected `world`" or "row 2 lost its bold green",
//! never "byte 0x1b differs at offset 1207". Text and style differences are
//! reported separately: a style-only regression on identical text is one of
//! the failure modes byte diffs are worst at explaining.

use crate::screen::char_width;
use crate::snapshot::{Frame, Snapshot, Span};
use std::fmt::Write as _;

/// One observed difference between an expected and an actual snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Difference {
    /// Grids of different sizes are never comparable row-by-row.
    Dims {
        expected: (usize, usize),
        actual: (usize, usize),
    },
    /// Row text differs (0-based row).
    RowText {
        row: usize,
        expected: String,
        actual: String,
    },
    /// Same text, different styling on a row.
    RowStyle {
        row: usize,
        expected: Vec<Span>,
        actual: Vec<Span>,
    },
    ExitCode {
        expected: i32,
        actual: i32,
    },
    Stderr {
        expected: Vec<String>,
        actual: Vec<String>,
    },
}

/// Compare two frames. Returns an empty vector when visually identical.
pub fn diff_frames(expected: &Frame, actual: &Frame) -> Vec<Difference> {
    let mut diffs = Vec::new();
    if (expected.cols, expected.rows) != (actual.cols, actual.rows) {
        diffs.push(Difference::Dims {
            expected: (expected.cols, expected.rows),
            actual: (actual.cols, actual.rows),
        });
        return diffs; // row-by-row comparison would be meaningless
    }
    for row in 0..expected.rows {
        if expected.lines[row] != actual.lines[row] {
            diffs.push(Difference::RowText {
                row,
                expected: expected.lines[row].clone(),
                actual: actual.lines[row].clone(),
            });
            continue; // style diffs on changed text are noise
        }
        let exp_spans: Vec<Span> = spans_of_row(expected, row);
        let act_spans: Vec<Span> = spans_of_row(actual, row);
        if exp_spans != act_spans {
            diffs.push(Difference::RowStyle {
                row,
                expected: exp_spans,
                actual: act_spans,
            });
        }
    }
    diffs
}

/// Compare two full snapshots: screen, then exit code, then stderr.
pub fn diff_snapshots(expected: &Snapshot, actual: &Snapshot) -> Vec<Difference> {
    let mut diffs = diff_frames(&expected.frame, &actual.frame);
    if expected.exit != actual.exit {
        diffs.push(Difference::ExitCode {
            expected: expected.exit,
            actual: actual.exit,
        });
    }
    if expected.stderr != actual.stderr {
        diffs.push(Difference::Stderr {
            expected: expected.stderr.clone(),
            actual: actual.stderr.clone(),
        });
    }
    diffs
}

fn spans_of_row(frame: &Frame, row: usize) -> Vec<Span> {
    frame
        .spans
        .iter()
        .filter(|s| s.row == row)
        .cloned()
        .collect()
}

/// Render differences as an indented, plain-text report (one difference per
/// block). `indent` is prepended to every line.
pub fn render_report(diffs: &[Difference], indent: &str) -> String {
    let mut out = String::new();
    for d in diffs {
        match d {
            Difference::Dims { expected, actual } => {
                let _ = writeln!(
                    out,
                    "{indent}terminal size: expected {}x{}, actual {}x{}",
                    expected.0, expected.1, actual.0, actual.1
                );
            }
            Difference::RowText {
                row,
                expected,
                actual,
            } => {
                let _ = writeln!(out, "{indent}row {row} text differs:");
                let _ = writeln!(out, "{indent}  expected |{expected}");
                let _ = writeln!(out, "{indent}  actual   |{actual}");
                let _ = writeln!(out, "{indent}            {}", caret_line(expected, actual));
            }
            Difference::RowStyle {
                row,
                expected,
                actual,
            } => {
                let _ = writeln!(out, "{indent}row {row} styles differ (text identical):");
                let _ = writeln!(out, "{indent}  expected {}", format_spans(expected));
                let _ = writeln!(out, "{indent}  actual   {}", format_spans(actual));
            }
            Difference::ExitCode { expected, actual } => {
                let _ = writeln!(
                    out,
                    "{indent}exit code: expected {expected}, actual {actual}"
                );
            }
            Difference::Stderr { expected, actual } => {
                let _ = writeln!(
                    out,
                    "{indent}stderr differs ({} line(s) expected, {} actual):",
                    expected.len(),
                    actual.len()
                );
                for line in first_stderr_disagreement(expected, actual) {
                    let _ = writeln!(out, "{indent}  {line}");
                }
            }
        }
    }
    out
}

/// A `^`-marker line under the actual row, aligned in *display columns* so
/// carets line up even past CJK characters. Marks every differing column.
fn caret_line(expected: &str, actual: &str) -> String {
    let mut out = String::new();
    let mut exp = expected.chars();
    let mut act = actual.chars();
    loop {
        match (exp.next(), act.next()) {
            (None, None) => break,
            (e, a) => {
                let width = char_width(a.or(e).unwrap_or(' ')).max(1) as usize;
                let mark = if e == a { ' ' } else { '^' };
                for _ in 0..width {
                    out.push(mark);
                }
            }
        }
    }
    out.truncate(out.trim_end().len());
    out
}

fn format_spans(spans: &[Span]) -> String {
    if spans.is_empty() {
        return "(plain)".to_string();
    }
    spans
        .iter()
        .map(|s| format!("c{}-c{} {}", s.start, s.end, s.style))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Show the first line where the stderr transcripts disagree, with context.
fn first_stderr_disagreement(expected: &[String], actual: &[String]) -> Vec<String> {
    let n = expected.len().max(actual.len());
    for i in 0..n {
        let e = expected.get(i);
        let a = actual.get(i);
        if e != a {
            return vec![
                format!(
                    "line {i}: expected {}",
                    e.map_or("(absent)".to_string(), |l| format!("|{l}"))
                ),
                format!(
                    "line {i}: actual   {}",
                    a.map_or("(absent)".to_string(), |l| format!("|{l}"))
                ),
            ];
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::Screen;
    use crate::snapshot::Frame;

    fn frame(cols: usize, rows: usize, input: &[u8]) -> Frame {
        let mut s = Screen::new(cols, rows);
        s.feed_bytes(input);
        Frame::from_screen(&s)
    }

    fn snap(frame: Frame, exit: i32, stderr: &[&str]) -> Snapshot {
        Snapshot {
            cmd: vec!["true".into()],
            exit,
            frame,
            stderr: stderr.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn identical_frames_produce_no_diffs() {
        let a = frame(20, 3, b"\x1b[32mok\x1b[0m done");
        let b = frame(20, 3, b"\x1b[32mok\x1b[0m done");
        assert!(diff_frames(&a, &b).is_empty());
    }

    #[test]
    fn different_escape_bytes_same_screen_is_equal() {
        // The whole point of the tool: `1;31` vs `31;1`, redundant resets and
        // a repaint must all compare equal when the rendered screen matches.
        let a = frame(20, 2, b"\x1b[1;31mred\x1b[0m");
        let b = frame(20, 2, b"\x1b[31m\x1b[1mred\x1b[m\x1b[0m");
        let c = frame(20, 2, b"XXX\r\x1b[2K\x1b[31;1mred\x1b[39;22m");
        assert!(diff_frames(&a, &b).is_empty());
        assert!(diff_frames(&a, &c).is_empty());
    }

    #[test]
    fn text_change_is_reported_with_row_number() {
        let a = frame(20, 3, b"one\r\ntwo");
        let b = frame(20, 3, b"one\r\ntwx");
        let d = diff_frames(&a, &b);
        assert_eq!(d.len(), 1);
        match &d[0] {
            Difference::RowText { row, .. } => assert_eq!(*row, 1),
            other => panic!("expected RowText, got {other:?}"),
        }
    }

    #[test]
    fn style_only_change_is_its_own_difference_kind() {
        let a = frame(20, 1, b"\x1b[32mPASS\x1b[0m");
        let b = frame(20, 1, b"\x1b[31mPASS\x1b[0m");
        let d = diff_frames(&a, &b);
        assert_eq!(d.len(), 1);
        assert!(matches!(d[0], Difference::RowStyle { row: 0, .. }));
    }

    #[test]
    fn text_change_suppresses_style_noise_on_that_row() {
        let a = frame(20, 1, b"\x1b[32mPASS\x1b[0m");
        let b = frame(20, 1, b"\x1b[31mFAIL\x1b[0m");
        let d = diff_frames(&a, &b);
        assert_eq!(d.len(), 1, "one RowText, no extra RowStyle");
        assert!(matches!(d[0], Difference::RowText { .. }));
    }

    #[test]
    fn dims_mismatch_short_circuits() {
        let a = frame(20, 3, b"x");
        let b = frame(10, 3, b"x");
        let d = diff_frames(&a, &b);
        assert_eq!(d.len(), 1);
        assert!(matches!(d[0], Difference::Dims { .. }));
    }

    #[test]
    fn caret_marks_differing_columns_display_width_aware() {
        assert_eq!(caret_line("hello world", "hello wurld"), "       ^");
        assert_eq!(caret_line("abc", "abcd"), "   ^");
        assert_eq!(caret_line("abcd", "abc"), "   ^");
        assert_eq!(caret_line("same", "same"), "");
        // The CJK char occupies two columns; the caret must land after both.
        assert_eq!(caret_line("字ab", "字ax"), "   ^");
    }

    #[test]
    fn exit_code_and_stderr_diffs() {
        let f = frame(10, 1, b"hi");
        let a = snap(f.clone(), 0, &["w1"]);
        let b = snap(f, 3, &["w1", "w2"]);
        let d = diff_snapshots(&a, &b);
        assert_eq!(d.len(), 2);
        assert!(matches!(
            d[0],
            Difference::ExitCode {
                expected: 0,
                actual: 3
            }
        ));
        assert!(matches!(d[1], Difference::Stderr { .. }));
    }

    #[test]
    fn report_renders_all_difference_kinds() {
        let f = frame(10, 1, b"\x1b[31mhi\x1b[0m");
        let g = frame(10, 1, b"\x1b[32mhi\x1b[0m");
        let mut diffs = diff_frames(&f, &g);
        diffs.push(Difference::ExitCode {
            expected: 0,
            actual: 1,
        });
        diffs.push(Difference::Stderr {
            expected: vec![],
            actual: vec!["boom".into()],
        });
        let report = render_report(&diffs, "  ");
        assert!(report.contains("styles differ"));
        assert!(report.contains("fg=red"));
        assert!(report.contains("fg=green"));
        assert!(report.contains("exit code: expected 0, actual 1"));
        assert!(report.contains("line 0: actual   |boom"));
    }
}
