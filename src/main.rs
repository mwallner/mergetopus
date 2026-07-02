mod cli;
mod color;
mod commands;
pub(crate) mod forges;
mod git_ops;
mod helpers;
mod models;
mod planner;
mod tui;
pub(crate) mod tui_progress;

#[cfg(test)]
mod test_support;

use anyhow::Result;
use clap::Parser;

use crate::cli::Args;
use crate::color::ColorConfig;

fn main() -> Result<()> {
    let args = Args::parse();
    
    // Initialize color configuration based on --color flag
    let color_config = ColorConfig::new(args.color);
    color::init_config(color_config);
    
    commands::run(args)
}
