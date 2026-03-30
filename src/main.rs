mod cli;
mod commands;
mod git_ops;
mod helpers;
mod models;
mod planner;
mod tui;

#[cfg(test)]
mod test_support;

use anyhow::Result;
use clap::Parser;

use crate::cli::Args;

fn main() -> Result<()> {
    let args = Args::parse();
    commands::run(args)
}
