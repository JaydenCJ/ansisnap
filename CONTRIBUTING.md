# Contributing to ansisnap

Thanks for your interest in improving ansisnap. Issues, discussions and pull requests are all welcome.

## Getting started

Prerequisites: Rust 1.75 or newer (stable toolchain).

```bash
git clone https://github.com/JaydenCJ/ansisnap.git
cd ansisnap
cargo build
cargo test
bash scripts/smoke.sh
```

`scripts/smoke.sh` records, breaks, diffs and re-blesses a real snapshot end to end in a temp directory. It finishes in well under a minute and must print `SMOKE OK`.

## Before you open a pull request

1. `cargo fmt` — formatting is enforced.
2. `cargo clippy --all-targets -- -D warnings` — clippy must be clean.
3. `cargo test` — unit tests and the CLI integration tests must pass.
4. `bash scripts/smoke.sh` — the smoke test must print `SMOKE OK`.
5. Add tests for behavior changes. The emulator lives in pure modules (`parser`, `style`, `screen`, `snapshot`, `differ`) that are easy to unit-test; please keep it that way.

## Ground rules

- Keep dependencies minimal. ansisnap currently has **zero** runtime dependencies; adding one needs a very strong justification in the PR description.
- No network calls, no telemetry. ansisnap runs the user's command and reads/writes local files — nothing else.
- Code comments and doc comments are written in English.
- Compatibility first: the `.snap` format is versioned by its first line. Behavior-visible emulator changes (new sequences, width-table updates) need a note in the changelog, because they can change what existing snapshots assert.

## Reporting bugs

Please include the `ansisnap --version` output, the exact escape-sequence input (e.g. `command | xxd | head`, or the raw capture file), the terminal size used, and what a real terminal shows versus what `ansisnap render` shows. Emulator bugs are much easier to fix with a minimal byte sequence that reproduces the wrong grid.

## Security

If you find a security issue (e.g. snapshot-path or command-handling related), please do not open a public issue. Use GitHub's private vulnerability reporting on this repository instead.
