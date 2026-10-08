use anyhow::Result;

mod cmd_cleanup;
mod cmd_consolidate;
mod cmd_discard;
mod cmd_here;
mod cmd_license;
mod cmd_merge_workflow;
pub(crate) mod cmd_pr;
mod cmd_push;
mod cmd_resolve;
pub(crate) mod cmd_status;
mod cmd_verify;

use crate::cli::{Args, Commands, PrSubcommand};
use mergetopus::color;
use mergetopus::git_ops;

/// Pick the concrete merge base Mergetopus anchors slicing, slice branches
/// and 3-way inputs to.
///
/// With a single best common ancestor this is simply `git merge-base A B`.
/// Criss-cross histories have multiple bases; git itself then merges against
/// a *virtual* base built from all of them, but Mergetopus needs one concrete
/// commit. The base `git merge-base A B` reports is offered as the default
/// (matching what forge PR bases show); interactively the user can pick a
/// different one, non-interactively a warning is printed and the default is
/// used.
pub(crate) fn select_merge_base(
    a: &str,
    b: &str,
    non_interactive: bool,
    tui_title: &str,
) -> Result<String> {
    let default = git_ops::merge_base(a, b)?;
    let all = git_ops::merge_bases(a, b)?;
    if all.len() <= 1 {
        return Ok(default);
    }

    let describe = |sha: &str| -> String {
        git_ops::run_git(&["log", "-1", "--format=%h %s", sha])
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| sha.chars().take(8).collect())
    };

    if non_interactive {
        color::print_warning(
            &format!(
                "{} merge bases between the merge sides (criss-cross history); git used a virtual base built from all of them. Using '{}' — the 'git merge-base' default — for slicing and 3-way inputs; run interactively to pick a different base.",
                all.len(),
                describe(&default)
            ),
            None,
        );
        return Ok(default);
    }

    let mut ordered: Vec<String> = Vec::new();
    let mut labels: Vec<String> = Vec::new();
    ordered.push(default.clone());
    labels.push(format!(
        "{}  — default ('git merge-base A B')",
        describe(&default)
    ));
    for sha in all.iter().filter(|s| **s != default) {
        ordered.push(sha.clone());
        labels.push(describe(sha));
    }

    let prompt = format!(
        "The histories criss-cross: there are {} merge bases. git merged against a virtual base combining them; Mergetopus anchors slices and 3-way views to ONE concrete base. Which should it use?",
        all.len()
    );
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let picked = crate::tui::pick_option(&prompt, &refs, tui_title)?;
    Ok(picked.map(|i| ordered[i].clone()).unwrap_or(default))
}

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

    if let Some(Commands::Resolve {
        branch,
        commit,
        on_group,
    }) = &args.command
    {
        let (_, tui_title) = current_branch_and_tui_title_worktree()?;
        return cmd_resolve::resolve_command(
            branch.as_deref(),
            *commit,
            args.quiet,
            *on_group,
            &tui_title,
        );
    }

    if let Some(Commands::Status { source, pr }) = &args.command {
        let (current_branch, tui_title) = current_branch_and_tui_title_worktree()?;
        return cmd_status::status_command(
            source.as_deref(),
            *pr,
            args.quiet,
            &current_branch,
            &tui_title,
        );
    }

    if let Some(Commands::Cleanup { close_prs }) = &args.command {
        let (current_branch, tui_title) = current_branch_and_tui_title_worktree()?;
        return cmd_cleanup::cleanup_command(*close_prs, args.quiet, &current_branch, &tui_title);
    }

    if let Some(Commands::Discard {
        integration,
        close_prs,
    }) = &args.command
    {
        let (current_branch, tui_title) = current_branch_and_tui_title_worktree()?;
        return cmd_discard::discard_command(
            integration.as_deref(),
            *close_prs,
            args.quiet,
            args.yes,
            &current_branch,
            &tui_title,
        );
    }

    if let Some(Commands::Verify { source, global }) = &args.command {
        let (current_branch, _) = current_branch_and_tui_title_worktree()?;
        return cmd_verify::verify_command(source.as_deref(), *global, &current_branch);
    }

    if let Some(Commands::Consolidate { source }) = &args.command {
        let (current_branch, _) = current_branch_and_tui_title_clean_context()?;
        return cmd_consolidate::consolidate_command(
            source.as_deref(),
            args.quiet,
            &current_branch,
        );
    }

    if let Some(Commands::Here { source }) = &args.command {
        let (current_branch, tui_title) = current_branch_and_tui_title_worktree()?;
        return cmd_here::here_command(&args, source.as_deref(), &current_branch, &tui_title);
    }

    if let Some(Commands::Push { remote, pr }) = &args.command {
        let (current_branch, tui_title) = current_branch_and_tui_title_worktree()?;
        return cmd_push::push_command(
            remote.as_deref(),
            *pr,
            args.quiet,
            &current_branch,
            &tui_title,
        );
    }

    if let Some(Commands::Pr { action }) = &args.command {
        let (current_branch, tui_title) = current_branch_and_tui_title_worktree()?;

        let (pr_action, source) = match action {
            PrSubcommand::Create { source } => (cmd_pr::PrAction::Create, source.as_deref()),
            PrSubcommand::Sync { source } => (cmd_pr::PrAction::Sync, source.as_deref()),
            PrSubcommand::List { source } => (cmd_pr::PrAction::List, source.as_deref()),
        };

        return cmd_pr::pr_command(pr_action, source, args.quiet, &current_branch, &tui_title);
    }

    // if we get to this point, it means we're starting or selecting integration with the "mergetopus <source>" command
    let (current_branch, tui_title) = current_branch_and_tui_title_clean_context()?;

    cmd_merge_workflow::run_merge_workflow(&args, &current_branch, &tui_title)
}
