//! ansisnap — snapshot-test CLI and TUI output through a built-in terminal
//! emulator.
//!
//! The pipeline: raw bytes are tokenized by [`parser`] into terminal actions,
//! [`screen`] applies them to a fixed-size cell grid (the same way a real
//! terminal would: cursor movement, scrolling, erases, styles), [`snapshot`]
//! serializes that grid into a reviewable text format, and [`differ`] compares
//! two grids cell by cell so a failing test reports *what changed on screen*,
//! never a wall of escape bytes.

pub mod cli;
pub mod differ;
pub mod parser;
pub mod runner;
pub mod screen;
pub mod snapshot;
pub mod style;

/// Package version, single source of truth for `--version` and headers.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
