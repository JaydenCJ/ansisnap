//! Command-line interface: argument parsing and the five subcommands.
//!
//! Exit codes follow the diff convention: `0` success / screens identical,
//! `1` a check failed or screens differ, `2` usage or I/O error. Argument
//! parsing is hand-rolled and pure (see [`Opts::parse`]) so it stays
//! unit-testable without spawning a process.

use crate::differ::{diff_frames, diff_snapshots, render_report};
use crate::runner::{self, stderr_lines};
use crate::screen::Screen;
use crate::snapshot::{encode_cmd, Frame, Snapshot};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const DEFAULT_DIR: &str = ".ansisnap";
pub const DEFAULT_COLS: usize = 80;
pub const DEFAULT_ROWS: usize = 24;

const HELP: &str = "\
ansisnap — snapshot-test CLI and TUI output as rendered screens

USAGE:
    ansisnap <command> [options]

COMMANDS:
    record <name> -- <cmd...>   run a command and store its rendered screen
    check [name...]             re-run recorded commands and compare screens
    render [file]               render ANSI bytes (file or stdin) to plain text
    diff <a> <b>                compare two snapshots or raw ANSI captures
    list                        list recorded snapshots

OPTIONS:
    --dir <path>     snapshot directory (default: .ansisnap)
    --cols <n>       terminal width for record/render/diff (default: 80)
    --rows <n>       terminal height (default: 24)
    --styles         render: also print style spans
    --update         check: rewrite snapshots that no longer match
    -h, --help       print this help
    -V, --version    print version

EXIT CODES:
    0 success | 1 screens differ or check failed | 2 usage or I/O error
";

/// Parsed command line, before dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opts {
    pub command: Cmd,
    pub dir: PathBuf,
    pub cols: usize,
    pub rows: usize,
    pub styles: bool,
    pub update: bool,
    pub positional: Vec<String>,
    /// The command after `--` (record only).
    pub child_cmd: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Record,
    Check,
    Render,
    Diff,
    List,
    Help,
    Version,
}

impl Opts {
    /// Pure argument parser. Returns a usage error string on bad input.
    pub fn parse(args: &[String]) -> Result<Opts, String> {
        let mut opts = Opts {
            command: Cmd::Help,
            dir: PathBuf::from(DEFAULT_DIR),
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            styles: false,
            update: false,
            positional: Vec::new(),
            child_cmd: Vec::new(),
        };
        let mut it = args.iter().peekable();
        let cmd = match it.next() {
            None => return Ok(opts),
            Some(c) => c.as_str(),
        };
        opts.command = match cmd {
            "record" => Cmd::Record,
            "check" => Cmd::Check,
            "render" => Cmd::Render,
            "diff" => Cmd::Diff,
            "list" => Cmd::List,
            "help" | "-h" | "--help" => Cmd::Help,
            "version" | "-V" | "--version" => Cmd::Version,
            other => return Err(format!("unknown command `{other}` (see --help)")),
        };
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--" => {
                    if opts.command != Cmd::Record {
                        return Err("`--` is only used by `record`".into());
                    }
                    opts.child_cmd = it.by_ref().cloned().collect();
                    break;
                }
                "--dir" => opts.dir = PathBuf::from(take_value(&mut it, "--dir")?),
                "--cols" => opts.cols = take_num(&mut it, "--cols")?,
                "--rows" => opts.rows = take_num(&mut it, "--rows")?,
                "--styles" => opts.styles = true,
                "--update" => opts.update = true,
                "-h" | "--help" => opts.command = Cmd::Help,
                "-V" | "--version" => opts.command = Cmd::Version,
                a if a.starts_with('-') && a.len() > 1 => {
                    return Err(format!("unknown option `{a}` (see --help)"))
                }
                _ => opts.positional.push(arg.clone()),
            }
        }
        Ok(opts)
    }
}

fn take_value<'a, I: Iterator<Item = &'a String>>(
    it: &mut std::iter::Peekable<I>,
    flag: &str,
) -> Result<String, String> {
    it.next()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn take_num<'a, I: Iterator<Item = &'a String>>(
    it: &mut std::iter::Peekable<I>,
    flag: &str,
) -> Result<usize, String> {
    let v = take_value(it, flag)?;
    let n: usize = v
        .parse()
        .map_err(|_| format!("{flag} needs a number, got `{v}`"))?;
    if (1..=1000).contains(&n) {
        Ok(n)
    } else {
        Err(format!("{flag} must be between 1 and 1000"))
    }
}

/// Snapshot names become file names: restrict them to a safe alphabet so a
/// name can never escape the snapshot directory.
pub fn validate_name(name: &str) -> Result<(), String> {
    let ok_first = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric());
    let ok_rest = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok_first && ok_rest {
        Ok(())
    } else {
        Err(format!(
            "invalid snapshot name `{name}` (use letters, digits, `.`, `_`, `-`; must start with a letter or digit)"
        ))
    }
}

/// Entry point used by `main`. Returns the process exit code.
pub fn run(args: &[String]) -> u8 {
    let opts = match Opts::parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("ansisnap: {e}");
            return 2;
        }
    };
    let result = match opts.command {
        Cmd::Help => {
            print!("{HELP}");
            Ok(0)
        }
        Cmd::Version => {
            println!("ansisnap {}", crate::VERSION);
            Ok(0)
        }
        Cmd::Record => cmd_record(&opts),
        Cmd::Check => cmd_check(&opts),
        Cmd::Render => cmd_render(&opts),
        Cmd::Diff => cmd_diff(&opts),
        Cmd::List => cmd_list(&opts),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("ansisnap: {e}");
            2
        }
    }
}

/// Run the command and render it into a [`Snapshot`].
fn capture(cmd: &[String], cols: usize, rows: usize) -> Result<Snapshot, String> {
    let out = runner::run(cmd, cols, rows)?;
    let mut screen = Screen::new(cols, rows);
    screen.feed_bytes(&out.stdout);
    Ok(Snapshot {
        cmd: cmd.to_vec(),
        exit: out.exit,
        frame: Frame::from_screen(&screen),
        stderr: stderr_lines(&out.stderr),
    })
}

fn snap_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.snap"))
}

fn cmd_record(opts: &Opts) -> Result<u8, String> {
    let name = match opts.positional.as_slice() {
        [name] => name,
        _ => return Err("usage: ansisnap record [options] <name> -- <command...>".into()),
    };
    validate_name(name)?;
    if opts.child_cmd.is_empty() {
        return Err("record needs a command after `--`".into());
    }
    let snap = capture(&opts.child_cmd, opts.cols, opts.rows)?;
    std::fs::create_dir_all(&opts.dir)
        .map_err(|e| format!("cannot create {}: {e}", opts.dir.display()))?;
    let path = snap_path(&opts.dir, name);
    std::fs::write(&path, snap.to_text())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    println!(
        "recorded {name} -> {} (exit {}, {}x{}, {} row(s) used, {} styled span(s))",
        path.display(),
        snap.exit,
        snap.frame.cols,
        snap.frame.rows,
        snap.frame.used_rows(),
        snap.frame.spans.len()
    );
    Ok(0)
}

fn load_snapshot(path: &Path) -> Result<Snapshot, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Snapshot::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Names to check: explicit positionals, else every `*.snap` in the dir,
/// sorted for stable output.
fn checkable_names(opts: &Opts) -> Result<Vec<String>, String> {
    if !opts.positional.is_empty() {
        for n in &opts.positional {
            validate_name(n)?;
        }
        return Ok(opts.positional.clone());
    }
    let mut names = Vec::new();
    let entries = std::fs::read_dir(&opts.dir)
        .map_err(|e| format!("cannot read {}: {e}", opts.dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let file = entry.file_name();
        if let Some(name) = file.to_string_lossy().strip_suffix(".snap") {
            names.push(name.to_string());
        }
    }
    names.sort();
    if names.is_empty() {
        return Err(format!("no snapshots in {}", opts.dir.display()));
    }
    Ok(names)
}

fn cmd_check(opts: &Opts) -> Result<u8, String> {
    let names = checkable_names(opts)?;
    let (mut ok, mut failed, mut updated) = (0usize, 0usize, 0usize);
    for name in &names {
        let path = snap_path(&opts.dir, name);
        let expected = load_snapshot(&path)?;
        let actual = capture(&expected.cmd, expected.frame.cols, expected.frame.rows)?;
        let diffs = diff_snapshots(&expected, &actual);
        if diffs.is_empty() {
            println!("ok      {name}");
            ok += 1;
        } else if opts.update {
            std::fs::write(&path, actual.to_text())
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            println!("updated {name} ({} difference(s) re-recorded)", diffs.len());
            updated += 1;
        } else {
            println!("FAIL    {name}");
            print!("{}", render_report(&diffs, "        "));
            failed += 1;
        }
    }
    let mut summary = format!("{} snapshot(s): {ok} ok", names.len());
    if failed > 0 {
        summary.push_str(&format!(", {failed} failed"));
    }
    if updated > 0 {
        summary.push_str(&format!(", {updated} updated"));
    }
    println!("{summary}");
    Ok(if failed > 0 { 1 } else { 0 })
}

fn read_input(path: Option<&String>) -> Result<Vec<u8>, String> {
    match path {
        Some(p) => std::fs::read(p).map_err(|e| format!("cannot read {p}: {e}")),
        None => {
            let mut buf = Vec::new();
            std::io::stdin()
                .read_to_end(&mut buf)
                .map_err(|e| format!("cannot read stdin: {e}"))?;
            Ok(buf)
        }
    }
}

fn cmd_render(opts: &Opts) -> Result<u8, String> {
    if opts.positional.len() > 1 {
        return Err("usage: ansisnap render [options] [file]".into());
    }
    let bytes = read_input(opts.positional.first())?;
    let mut screen = Screen::new(opts.cols, opts.rows);
    screen.feed_bytes(&bytes);
    let frame = Frame::from_screen(&screen);
    for line in frame.lines.iter().take(frame.used_rows()) {
        println!("{line}");
    }
    if opts.styles {
        let n = frame.spans.len();
        println!(
            "--- styles: {n} {} ---",
            crate::snapshot::plural(n, "span", "spans")
        );
        for s in &frame.spans {
            println!("r{} c{}-c{} {}", s.row, s.start, s.end, s.style);
        }
    }
    Ok(0)
}

/// Load a diff operand: an ansisnap snapshot file is parsed; anything else
/// is treated as raw ANSI bytes and rendered at the requested size.
fn load_operand(path: &str, cols: usize, rows: usize) -> Result<Frame, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    if bytes.starts_with(b"ansisnap snapshot v1\n") {
        let text = String::from_utf8_lossy(&bytes);
        let snap = Snapshot::parse(&text).map_err(|e| format!("{path}: {e}"))?;
        return Ok(snap.frame);
    }
    let mut screen = Screen::new(cols, rows);
    screen.feed_bytes(&bytes);
    Ok(Frame::from_screen(&screen))
}

fn cmd_diff(opts: &Opts) -> Result<u8, String> {
    let (a, b) = match opts.positional.as_slice() {
        [a, b] => (a, b),
        _ => return Err("usage: ansisnap diff [options] <a> <b>".into()),
    };
    let fa = load_operand(a, opts.cols, opts.rows)?;
    let fb = load_operand(b, opts.cols, opts.rows)?;
    let diffs = diff_frames(&fa, &fb);
    if diffs.is_empty() {
        println!("screens identical ({}x{})", fa.cols, fa.rows);
        return Ok(0);
    }
    println!("{} difference(s) between {a} and {b}:", diffs.len());
    print!("{}", render_report(&diffs, "  "));
    Ok(1)
}

fn cmd_list(opts: &Opts) -> Result<u8, String> {
    let names = checkable_names(opts)?;
    for name in &names {
        let snap = load_snapshot(&snap_path(&opts.dir, name))?;
        println!(
            "{name}\t{}x{}\texit {}\t{}",
            snap.frame.cols,
            snap.frame.rows,
            snap.exit,
            encode_cmd(&snap.cmd)
        );
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Opts, String> {
        let v: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        Opts::parse(&v)
    }

    #[test]
    fn no_args_means_help_and_version_flag_wins_anywhere() {
        assert_eq!(parse(&[]).unwrap().command, Cmd::Help);
        assert_eq!(parse(&["--version"]).unwrap().command, Cmd::Version);
        assert_eq!(parse(&["check", "-V"]).unwrap().command, Cmd::Version);
    }

    #[test]
    fn record_splits_child_command_at_double_dash() {
        let o = parse(&[
            "record",
            "--cols",
            "40",
            "demo",
            "--",
            "ls",
            "--color=always",
        ])
        .unwrap();
        assert_eq!(o.command, Cmd::Record);
        assert_eq!(o.cols, 40);
        assert_eq!(o.positional, vec!["demo"]);
        assert_eq!(o.child_cmd, vec!["ls", "--color=always"]);
    }

    #[test]
    fn flags_after_double_dash_belong_to_the_child() {
        // `--update` after `--` must NOT be eaten as an ansisnap flag.
        let o = parse(&["record", "x", "--", "tool", "--update"]).unwrap();
        assert!(!o.update);
        assert_eq!(o.child_cmd, vec!["tool", "--update"]);
    }

    #[test]
    fn unknown_command_option_and_stray_double_dash_are_usage_errors() {
        assert!(parse(&["explode"]).is_err());
        assert!(parse(&["check", "--frobnicate"]).is_err());
        assert!(parse(&["check", "--", "x"]).is_err());
    }

    #[test]
    fn dimension_flags_are_validated() {
        assert!(parse(&["render", "--cols", "0"]).is_err());
        assert!(parse(&["render", "--cols", "1001"]).is_err());
        assert!(parse(&["render", "--cols", "abc"]).is_err());
        assert!(parse(&["render", "--cols"]).is_err());
        assert_eq!(parse(&["render", "--rows", "3"]).unwrap().rows, 3);
    }

    #[test]
    fn defaults_are_80_by_24_in_dot_ansisnap() {
        let o = parse(&["check"]).unwrap();
        assert_eq!((o.cols, o.rows), (DEFAULT_COLS, DEFAULT_ROWS));
        assert_eq!(o.dir, PathBuf::from(DEFAULT_DIR));
    }

    #[test]
    fn name_validation_blocks_path_escapes() {
        assert!(validate_name("demo").is_ok());
        assert!(validate_name("demo-2.color_x").is_ok());
        assert!(validate_name("../evil").is_err());
        assert!(validate_name(".hidden").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("").is_err());
        assert!(validate_name("-flag").is_err());
    }
}
