# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-07-13

### Added

- Built-in terminal emulator: incremental VT/xterm escape parser (CSI/OSC/DCS, split-across-chunks reassembly, UTF-8 with replacement-char recovery) driving a fixed-size cell grid — cursor movement (CUP/CUU/CUD/CUF/CUB/CNL/CPL/CHA/VPA), erases (ED/EL/ECH), edits (ICH/DCH/IL/DL), scrolling with DECSTBM scroll regions, deferred autowrap, tab stops, save/restore cursor, the alternate screen buffer, and ONLCR emulation for pipe captures.
- Full SGR styling: bold/dim/italic/underline/blink/reverse/hidden/strike, 16-color, 256-color and 24-bit truecolor in both semicolon and colon forms, with low palette indices normalized so `38;5;1` and `31` compare equal.
- East Asian width handling: CJK/kana/Hangul/fullwidth/emoji occupy two cells, combining marks zero, so grids and diff carets stay aligned for Japanese and Chinese output.
- `ansisnap record <name> -- <cmd>`: runs the command with a pinned terminal environment (`TERM`, `COLUMNS`/`LINES`, `CLICOLOR_FORCE`, `FORCE_COLOR` set; `NO_COLOR` removed; locale pinned) and stores the rendered screen, style spans, stripped stderr and exit code as a reviewable `.snap` text file.
- `ansisnap check [--update]`: re-runs recorded commands and compares rendered screens cell-for-cell; failures report row-level text diffs with display-width-aware caret markers, style-only regressions ("text identical, green became red"), exit-code and stderr changes. `--update` re-blesses.
- `ansisnap render`: turn any captured ANSI byte stream (file or stdin) into the plain text a terminal would display, optionally with a style-span table.
- `ansisnap diff`: compare two snapshots or raw ANSI captures as screens — byte-different but visually identical streams compare equal.
- `ansisnap list`: enumerate recorded snapshots with dimensions, exit code and command.
- Versioned, positioned-error snapshot format (`ansisnap snapshot v1`): corrupt files fail with a line number instead of comparing garbage; snapshot names are restricted to a safe alphabet so they can never escape the snapshot directory.
- Test suite: 78 unit tests, 13 CLI integration tests against the compiled binary, and `scripts/smoke.sh`.

[0.1.0]: https://github.com/JaydenCJ/ansisnap/releases/tag/v0.1.0
