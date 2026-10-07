mod cli;
mod commands;
mod helpers;
mod tui;
pub(crate) mod tui_progress;

use anyhow::Result;
use clap::Parser;

use mergetopus::color::ColorConfig;

use crate::cli::Args;

fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize color configuration based on --color flag
    let color_config = ColorConfig::new(args.color);
    mergetopus::color::init_config(color_config);

    commands::run(args)
}
