use anyhow::Result;

mod cmd_cleanup;
mod cmd_here;
mod cmd_license;
mod cmd_resolve;
mod cmd_status;
mod cmd_merge_workflow;

use crate::cli::{Args, Commands};
use crate::git_ops;

pub fn run(args: Args) -> Result<()> {
    if let Some(Commands::License { full, json_output }) = &args.command {
        cmd_license::print_license(*full, *json_output);
        return Ok(());
    }

    if let Some(Commands::Resolve { branch, commit }) = &args.command {
        git_ops::ensure_git_worktree()?;
        let current_branch = git_ops::current_branch()?;
        let tui_title = format!("Mergetopus [{current_branch}]");
        return cmd_resolve::resolve_command(branch.as_deref(), *commit, args.quiet, &tui_title);
    }

    if let Some(Commands::Status { source }) = &args.command {
        git_ops::ensure_git_worktree()?;
        let current_branch = git_ops::current_branch()?;
        let tui_title = format!("Mergetopus [{current_branch}]");
        return cmd_status::status_command(
            source.as_deref(),
            args.quiet,
            &current_branch,
            &tui_title,
        );
    }

    if let Some(Commands::Cleanup) = &args.command {
        git_ops::ensure_git_worktree()?;
        let current_branch = git_ops::current_branch()?;
        let tui_title = format!("Mergetopus [{current_branch}]");
        return cmd_cleanup::cleanup_command(args.quiet, &current_branch, &tui_title);
    }

    if let Some(Commands::Here) = &args.command {
        git_ops::ensure_git_worktree()?;
        let current_branch = git_ops::current_branch()?;
        let tui_title = format!("Mergetopus [{current_branch}]");
        return cmd_here::here_command(&args, &current_branch, &tui_title);
    }

    // if we get to this point, it means we're starting or selecting integration with the "mergetopus <source>" command
    git_ops::ensure_git_context()?;
    let current_branch = git_ops::current_branch()?;
    let tui_title = format!("Mergetopus [{current_branch}]");

    cmd_merge_workflow::run_merge_workflow(&args, &current_branch, &tui_title)
}
