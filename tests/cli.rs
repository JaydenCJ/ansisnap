//! End-to-end tests against the compiled `ansisnap` binary.
//!
//! Each test gets its own temp directory (cleaned up on drop), runs the real
//! binary via `CARGO_BIN_EXE_ansisnap`, and asserts on stdout/stderr/exit
//! codes. Commands under test are tiny `sh` scripts, so everything is
//! offline and deterministic.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_ansisnap");

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A per-test scratch directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("ansisnap-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn file(&self, name: &str, content: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run ansisnap with `args` inside `dir`, optionally with bytes on stdin.
fn ansisnap_in(dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ansisnap");
    if let Some(bytes) = stdin {
        child.stdin.take().unwrap().write_all(bytes).unwrap();
    }
    child.wait_with_output().expect("wait for ansisnap")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn version_and_help() {
    let t = TempDir::new();
    let out = ansisnap_in(t.path(), &["--version"], None);
    assert_eq!(stdout(&out).trim(), "ansisnap 0.1.0");
    assert_eq!(out.status.code(), Some(0));

    let out = ansisnap_in(t.path(), &["--help"], None);
    let text = stdout(&out);
    assert!(text.contains("COMMANDS:"));
    assert!(text.contains("record"));
    assert!(text.contains("EXIT CODES:"));
}

#[test]
fn record_then_check_passes_despite_volatile_escape_bytes() {
    let t = TempDir::new();
    // A script whose byte stream is full of overdraws and redundant SGRs but
    // whose final screen is stable — the tool's core promise.
    t.file(
        "app.sh",
        "#!/bin/sh\n\
         printf 'working 1%%\\rworking 50%%\\rworking 99%%\\r\\033[2K'\n\
         printf '\\033[1m\\033[32mPASS\\033[0m 12 checks\\n'\n",
    );
    let out = ansisnap_in(t.path(), &["record", "app", "--", "sh", "app.sh"], None);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let line = stdout(&out);
    assert!(line.contains("recorded app -> .ansisnap/app.snap"));
    assert!(line.contains("exit 0"));

    // The stored screen contains the final frame, not the progress noise.
    let snap = std::fs::read_to_string(t.path().join(".ansisnap/app.snap")).unwrap();
    assert!(snap.contains("|PASS 12 checks"));
    assert!(!snap.contains("working"));
    assert!(snap.contains("bold,fg=green"));

    let out = ansisnap_in(t.path(), &["check"], None);
    assert_eq!(out.status.code(), Some(0), "stdout: {}", stdout(&out));
    assert!(stdout(&out).contains("ok      app"));
    assert!(stdout(&out).contains("1 snapshot(s): 1 ok"));
}

#[test]
fn check_fails_with_row_level_diff_on_text_change() {
    let t = TempDir::new();
    t.file("data.txt", "alpha\n");
    t.file("show.sh", "#!/bin/sh\ncat data.txt\n");
    ansisnap_in(t.path(), &["record", "show", "--", "sh", "show.sh"], None);

    t.file("data.txt", "aloha\n");
    let out = ansisnap_in(t.path(), &["check"], None);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("FAIL    show"));
    assert!(text.contains("row 0 text differs"));
    assert!(text.contains("expected |alpha"));
    assert!(text.contains("actual   |aloha"));
    assert!(text.contains("^"), "caret marker missing:\n{text}");
    assert!(text.contains("1 snapshot(s): 0 ok, 1 failed"));
}

#[test]
fn check_reports_style_only_regressions() {
    let t = TempDir::new();
    t.file("color.txt", "32");
    t.file(
        "st.sh",
        "#!/bin/sh\nprintf '\\033[%smSTATUS\\033[0m\\n' \"$(cat color.txt)\"\n",
    );
    ansisnap_in(t.path(), &["record", "st", "--", "sh", "st.sh"], None);

    t.file("color.txt", "31"); // green -> red, text identical
    let out = ansisnap_in(t.path(), &["check"], None);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("styles differ (text identical)"));
    assert!(text.contains("fg=green"));
    assert!(text.contains("fg=red"));
}

#[test]
fn check_update_reblesses_failing_snapshots() {
    let t = TempDir::new();
    t.file("msg.txt", "v1\n");
    t.file("m.sh", "#!/bin/sh\ncat msg.txt\n");
    ansisnap_in(t.path(), &["record", "m", "--", "sh", "m.sh"], None);

    t.file("msg.txt", "v2\n");
    let out = ansisnap_in(t.path(), &["check", "--update"], None);
    assert_eq!(out.status.code(), Some(0), "update run must not fail");
    assert!(stdout(&out).contains("updated m"));

    let out = ansisnap_in(t.path(), &["check"], None);
    assert_eq!(out.status.code(), Some(0));
    assert!(stdout(&out).contains("ok      m"));
}

#[test]
fn check_catches_exit_code_and_stderr_regressions() {
    let t = TempDir::new();
    t.file("rc.txt", "0");
    t.file(
        "e.sh",
        "#!/bin/sh\necho steady\nrc=$(cat rc.txt)\n[ \"$rc\" != 0 ] && echo 'boom' >&2\nexit \"$rc\"\n",
    );
    ansisnap_in(t.path(), &["record", "e", "--", "sh", "e.sh"], None);

    t.file("rc.txt", "3");
    let out = ansisnap_in(t.path(), &["check"], None);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("exit code: expected 0, actual 3"));
    assert!(text.contains("stderr differs"));
    assert!(text.contains("|boom"));
}

#[test]
fn render_collapses_progress_bars_from_stdin() {
    let t = TempDir::new();
    let out = ansisnap_in(
        t.path(),
        &["render"],
        Some(b"downloading 1%\rdownloading 62%\rdownloading 100%\r\x1b[2Kdone: 4 files\n"),
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "done: 4 files\n");
}

#[test]
fn render_file_with_styles_prints_span_table() {
    let t = TempDir::new();
    let f = t.file("cap.bin", "\x1b[7m INS \x1b[0m ready\n");
    let out = ansisnap_in(
        t.path(),
        &["render", "--styles", "--cols", "20", f.to_str().unwrap()],
        None,
    );
    let text = stdout(&out);
    assert!(text.contains(" INS  ready"));
    assert!(text.contains("--- styles: 1 span ---"));
    assert!(text.contains("r0 c0-c4 reverse"));
}

#[test]
fn diff_equates_different_bytes_with_the_same_screen() {
    let t = TempDir::new();
    // Same rendered result via completely different escape sequences.
    let a = t.file("a.out", "\x1b[1;31merror:\x1b[0m nope\n");
    let b = t.file(
        "b.out",
        "XXXXXX nope\r\x1b[31m\x1b[1merror:\x1b[22m\x1b[39m\n",
    );
    let out = ansisnap_in(
        t.path(),
        &["diff", a.to_str().unwrap(), b.to_str().unwrap()],
        None,
    );
    assert_eq!(out.status.code(), Some(0), "stdout: {}", stdout(&out));
    assert!(stdout(&out).contains("screens identical"));

    let c = t.file("c.out", "\x1b[1;31merror:\x1b[0m yep\n");
    let out = ansisnap_in(
        t.path(),
        &["diff", a.to_str().unwrap(), c.to_str().unwrap()],
        None,
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("row 0 text differs"));
}

#[test]
fn list_shows_dimensions_exit_and_command() {
    let t = TempDir::new();
    ansisnap_in(
        t.path(),
        &[
            "record", "--cols", "40", "--rows", "10", "hello", "--", "sh", "-c", "echo hi",
        ],
        None,
    );
    let out = ansisnap_in(t.path(), &["list"], None);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    assert!(text.contains("hello"));
    assert!(text.contains("40x10"));
    assert!(text.contains("exit 0"));
    assert!(text.contains(r#"["sh","-c","echo hi"]"#));
}

#[test]
fn tui_frame_on_alternate_screen_snapshots_final_state() {
    let t = TempDir::new();
    // Simulates a full-screen TUI: alt screen, cursor-addressed frame, then
    // exit back to the main screen leaving a summary line.
    t.file(
        "tui.sh",
        "#!/bin/sh\n\
         printf '\\033[?1049h\\033[2J\\033[1;1H+----+\\033[2;1H|body|\\033[3;1H+----+'\n\
         sleep 0\n\
         printf '\\033[?1049l'\n\
         printf 'session closed\\n'\n",
    );
    let out = ansisnap_in(t.path(), &["record", "tui", "--", "sh", "tui.sh"], None);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    let snap = std::fs::read_to_string(t.path().join(".ansisnap/tui.snap")).unwrap();
    assert!(snap.contains("|session closed"));
    assert!(!snap.contains("|body|"), "alt-screen frame must not leak");

    let out = ansisnap_in(t.path(), &["check"], None);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn usage_errors_exit_2() {
    let t = TempDir::new();
    let out = ansisnap_in(t.path(), &["explode"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("unknown command"));

    let out = ansisnap_in(t.path(), &["record", "../evil", "--", "true"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("invalid snapshot name"));

    let out = ansisnap_in(t.path(), &["check"], None);
    assert_eq!(out.status.code(), Some(2), "no snapshot dir yet");
}

#[test]
fn corrupt_snapshot_fails_with_positioned_error() {
    let t = TempDir::new();
    ansisnap_in(
        t.path(),
        &["record", "ok", "--", "sh", "-c", "echo x"],
        None,
    );
    let path = t.path().join(".ansisnap/ok.snap");
    let mangled = std::fs::read_to_string(&path)
        .unwrap()
        .replace("exit: ", "exit code: ");
    std::fs::write(&path, mangled).unwrap();
    let out = ansisnap_in(t.path(), &["check"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("line 4"), "stderr: {}", stderr(&out));
}
