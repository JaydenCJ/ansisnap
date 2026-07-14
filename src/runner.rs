//! Command execution with a deterministic terminal-like environment.
//!
//! The child runs with pipes, not a PTY (keeping ansisnap dependency-free and
//! portable), so tools that probe `isatty` would normally disable color. The
//! runner compensates the way CI color setups do: `TERM`, `COLUMNS`/`LINES`,
//! `CLICOLOR_FORCE` and `FORCE_COLOR` are set, `NO_COLOR` is removed. Locale
//! variables are pinned so number/message formatting cannot drift between the
//! machine that recorded a snapshot and the machine that checks it.

use std::io::Read;
use std::process::{Command, Stdio};

/// Everything captured from one run of the command under test.
#[derive(Debug)]
pub struct RunOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit: i32,
}

/// Exit code reported when the child was killed by a signal (no exit code
/// exists); chosen to match the shell convention of `128 + signal` being
/// out-of-band, without depending on platform signal numbers.
pub const SIGNAL_EXIT: i32 = -1;

/// Run `cmd` (argv vector) with the pinned environment for a `cols`x`rows`
/// terminal. Returns captured stdout/stderr bytes and the exit code.
pub fn run(cmd: &[String], cols: usize, rows: usize) -> Result<RunOutput, String> {
    let (prog, args) = cmd
        .split_first()
        .ok_or_else(|| "empty command".to_string())?;
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("TERM", "xterm-256color")
        .env("COLUMNS", cols.to_string())
        .env("LINES", rows.to_string())
        .env("CLICOLOR_FORCE", "1")
        .env("FORCE_COLOR", "1")
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .env_remove("NO_COLOR")
        .spawn()
        .map_err(|e| format!("cannot run `{prog}`: {e}"))?;

    // Drain both pipes concurrently: a child that fills the stderr pipe while
    // we block on stdout would deadlock otherwise.
    let mut stdout_pipe = child.stdout.take().expect("stdout piped");
    let stderr_pipe = child.stderr.take().expect("stderr piped");
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut pipe = stderr_pipe;
        let _ = pipe.read_to_end(&mut buf);
        buf
    });
    let mut stdout = Vec::new();
    stdout_pipe
        .read_to_end(&mut stdout)
        .map_err(|e| format!("reading stdout of `{prog}`: {e}"))?;
    let stderr = stderr_thread.join().unwrap_or_default();
    let status = child
        .wait()
        .map_err(|e| format!("waiting for `{prog}`: {e}"))?;
    Ok(RunOutput {
        stdout,
        stderr,
        exit: status.code().unwrap_or(SIGNAL_EXIT),
    })
}

/// Reduce a raw stderr byte stream to plain text lines: escape sequences are
/// parsed and dropped, `\r`-overwrites keep only the final state of the line
/// (progress bars on stderr are the norm), trailing blank lines are trimmed.
pub fn stderr_lines(bytes: &[u8]) -> Vec<String> {
    use crate::parser::{Action, Parser};
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut pending_cr = false;
    for action in Parser::parse(bytes) {
        match action {
            Action::Print(c) => {
                if pending_cr {
                    cur.clear();
                    pending_cr = false;
                }
                cur.push(c);
            }
            Action::Control(b'\n') => {
                pending_cr = false;
                cur.truncate(cur.trim_end().len());
                lines.push(std::mem::take(&mut cur));
            }
            Action::Control(b'\r') => pending_cr = true,
            Action::Control(b'\t') => {
                if pending_cr {
                    cur.clear();
                    pending_cr = false;
                }
                cur.push(' ');
            }
            _ => {} // cursor games on stderr: text content is what matters
        }
    }
    cur.truncate(cur.trim_end().len());
    if !cur.is_empty() {
        lines.push(cur);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Vec<String> {
        vec!["sh".into(), "-c".into(), script.into()]
    }

    #[test]
    fn captures_stdout_stderr_and_exit_code() {
        let out = run(&sh("echo out; echo err >&2; exit 3"), 80, 24).unwrap();
        assert_eq!(out.stdout, b"out\n");
        assert_eq!(out.stderr, b"err\n");
        assert_eq!(out.exit, 3);
    }

    #[test]
    fn missing_program_and_empty_command_are_clean_errors() {
        let e = run(&["ansisnap-no-such-tool-xyz".into()], 80, 24).unwrap_err();
        assert!(e.contains("cannot run"));
        assert!(run(&[], 80, 24).is_err());
    }

    #[test]
    fn terminal_environment_is_pinned() {
        let out = run(
            &sh("printf '%s %s %s %s' \"$TERM\" \"$COLUMNS\" \"$LINES\" \"$CLICOLOR_FORCE\""),
            132,
            50,
        )
        .unwrap();
        assert_eq!(out.stdout, b"xterm-256color 132 50 1");
    }

    #[test]
    fn no_color_is_stripped_from_the_child_env() {
        // Even if the recording machine exports NO_COLOR, checks elsewhere
        // must see the same environment.
        let out = run(&sh("printf '%s' \"${NO_COLOR:-unset}\""), 80, 24).unwrap();
        assert_eq!(out.stdout, b"unset");
    }

    #[test]
    fn large_stderr_does_not_deadlock() {
        // 256 KiB of stderr exceeds any pipe buffer; the drain thread must
        // keep the child from blocking.
        let out = run(
            &sh("i=0; while [ $i -lt 4096 ]; do printf '%064d\\n' $i >&2; i=$((i+1)); done"),
            80,
            24,
        )
        .unwrap();
        assert_eq!(out.exit, 0);
        assert_eq!(out.stderr.len(), 4096 * 65);
    }

    #[test]
    fn stderr_lines_strips_escapes_and_cr_overwrites() {
        let lines = stderr_lines(b"\x1b[31mwarn:\x1b[0m thing\nprogress 1%\rprogress 99%\rdone\n");
        assert_eq!(lines, vec!["warn: thing", "done"]);
        // `\r\n` must be a line break, not an overwrite of the line.
        let lines = stderr_lines(b"one\r\ntwo\r\n");
        assert_eq!(lines, vec!["one", "two"]);
        // Trailing blank lines are noise, not contract.
        let lines = stderr_lines(b"msg\n\n\n");
        assert_eq!(lines, vec!["msg"]);
    }
}
