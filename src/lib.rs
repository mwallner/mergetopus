//! mergetopus as a library.
//!
//! Exposes the pieces external consumers (biggit) need: pure branch-name
//! logic (`planner`), git plumbing (`git_ops`), forge lookups (`forges`) and
//! the data model (`models`). The TUI, CLI and command front ends stay in
//! the binary behind the `tui`/`cli` features.

pub mod color;
pub mod forges;
pub mod git_ops;
pub mod models;
pub mod planner;
pub mod win32_path;

#[cfg(test)]
mod test_support;
