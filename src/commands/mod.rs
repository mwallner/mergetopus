use anyhow::Result;

mod cmd_cleanup;
mod cmd_here;
mod cmd_license;
mod cmd_merge_workflow;
mod cmd_push;
mod cmd_resolve;
pub(crate) mod cmd_status;
mod cmd_verify;

use crate::cli::{Args, Commands};
use crate::git_ops;

fn current_branch_and_tui_title_worktree() -> Result<(String, String)> {
    git_ops::ensure_git_worktree()?;
    let current_branch = git_ops::current_branch()?;
    let tui_title = format!("Mergetopus [{current_branch}]");
    Ok((current_branch, tui_title))
}

fn current_branch_and_tui_title_clean_context() -> Result<(String, String)> {
    git_ops::ensure_git_context()?;
    let current_branch = git_ops::current_branch()?;
    let tui_title = format!("Mergetopus [{current_branch}]");
    Ok((current_branch, tui_title))
}

pub fn run(args: Args) -> Result<()> {
    if let Some(Commands::License { full, json_output }) = &args.command {
        cmd_license::print_license(*full, *json_output);
        return Ok(());
    }

    if let Some(Commands::Resolve { branch, commit }) = &args.command {
        let (_, tui_title) = current_branch_and_tui_title_worktree()?;
        return cmd_resolve::resolve_command(branch.as_deref(), *commit, args.quiet, &tui_title);
    }

    if let Some(Commands::Status { source }) = &args.command {
        let (current_branch, tui_title) = current_branch_and_tui_title_worktree()?;
        return cmd_status::status_command(
            source.as_deref(),
            args.quiet,
            &current_branch,
            &tui_title,
        );
    }

    if let Some(Commands::Cleanup) = &args.command {
        let (current_branch, tui_title) = current_branch_and_tui_title_worktree()?;
        return cmd_cleanup::cleanup_command(args.quiet, &current_branch, &tui_title);
    }

    if let Some(Commands::Verify { source, global }) = &args.command {
        let (current_branch, _) = current_branch_and_tui_title_worktree()?;
        return cmd_verify::verify_command(source.as_deref(), *global, &current_branch);
    }

    if let Some(Commands::Here) = &args.command {
        let (current_branch, tui_title) = current_branch_and_tui_title_worktree()?;
        return cmd_here::here_command(&args, &current_branch, &tui_title);
    }

    if let Some(Commands::Push { remote }) = &args.command {
        let (current_branch, tui_title) = current_branch_and_tui_title_worktree()?;
        return cmd_push::push_command(
            remote.as_deref(),
            args.quiet,
            &current_branch,
            &tui_title,
        );
    }

    // if we get to this point, it means we're starting or selecting integration with the "mergetopus <source>" command
    let (current_branch, tui_title) = current_branch_and_tui_title_clean_context()?;

    cmd_merge_workflow::run_merge_workflow(&args, &current_branch, &tui_title)
}
