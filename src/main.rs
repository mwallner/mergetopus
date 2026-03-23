mod cli;
mod commands;
mod git_ops;
mod license;
mod models;
mod planner;
mod tui;

use anyhow::Result;
use clap::Parser;

use crate::cli::Args;

fn main() -> Result<()> {
    let args = Args::parse();
    commands::run(args)
}
