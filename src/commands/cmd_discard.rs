use anyhow::{Result, bail};

use crate::tui;
use mergetopus::color;
use mergetopus::forges;
use mergetopus::forges::detect::{detect_forge, parse_remote_url};
use mergetopus::git_ops;
use mergetopus::planner;

/// Discard a Mergetopus workflow by deleting all associated branches.
///
/// Lists all branches belonging to a workflow (integration + slice branches),
/// shows them for confirmation, deletes local and optionally remote tracking
/// branches, and closes associated PRs.
pub fn discard_command(
    integration: Option<&str>,
    close_prs: bool,
    quiet: bool,
    yes: bool,
    current_branch: &str,
    tui_title: &str,
) -> Result<()> {
    let integration_branch = resolve_workflow(integration, quiet, current_branch, tui_title)?;

    let mut branches = git_ops::list_slice_branches_for_integration(&integration_branch)?;
    branches.insert(0, integration_branch.clone());

    // Determine which branches exist locally and on which remotes (by
    // parsing the actual tracking refs, so deletion targets the matching
    // remote instead of a guessed first one). Ref inventory is loaded
    // once; per-branch probing costs 2 subprocesses each on large runs.
    let locals: std::collections::BTreeSet<String> =
        git_ops::list_local_branches()?.into_iter().collect();
    let all_remote_refs = git_ops::list_remote_refs()?;
    let remote_names = git_ops::list_remote_names()?;
    let mut branch_info: Vec<(String, bool, Vec<String>)> = Vec::new();
    for branch in &branches {
        let local = locals.contains(branch);
        let mut remotes = Vec::new();
        let suffix = format!("/{branch}");
        for tracked_ref in all_remote_refs.iter().filter(|r| r.ends_with(&suffix)) {
            if let Some(remote) =
                git_ops::remote_for_tracking_ref_with(&remote_names, tracked_ref, branch)
                && !remotes.contains(&remote)
            {
                remotes.push(remote);
            }
        }
        branch_info.push((branch.clone(), local, remotes));
    }

    if branch_info
        .iter()
        .all(|(_, local, remotes)| !local && remotes.is_empty())
    {
        color::print_warning(
            &format!("No branches found for workflow '{integration_branch}'"),
            None,
        );
        return Ok(());
    }

    let display: Vec<String> = branch_info
        .iter()
        .map(|(name, local, remotes)| {
            let mut tags = Vec::new();
            if *local {
                tags.push("local".to_string());
            }
            for remote in remotes {
                tags.push(format!("remote:{remote}"));
            }
            format!("{name}  ({})", tags.join(", "))
        })
        .collect();

    if quiet && !yes {
        bail!(
            "discard requires interactive confirmation; re-run without --quiet to proceed, or use --yes to auto-confirm"
        );
    }

    let confirmed = yes
        || tui::confirm_list(
            &display,
            &format!(
                "Delete {} branch(es)? This cannot be undone.",
                display.len()
            ),
            tui_title,
        )?;

    if !confirmed {
        color::print_warning("Discard canceled.", None);
        return Ok(());
    }

    let mut deleted_local = 0usize;
    let mut deleted_remote = 0usize;

    // One linked-worktree probe for the whole deletion batch.
    let worktree_state = git_ops::WorktreeState::load()?;
    for (name, local, _remotes) in &branch_info {
        if *local {
            if name == current_branch {
                color::print_error(
                    &format!("Skipping '{name}': cannot delete the currently checked-out branch"),
                    None,
                );
                continue;
            }
            git_ops::delete_branch_with(&worktree_state, name)?;
            color::print_success(&format!("Deleted local: {name}"), None);
            deleted_local += 1;
        }
    }

    let has_remote_branches = branch_info
        .iter()
        .any(|(_, _, remotes)| !remotes.is_empty());

    if has_remote_branches {
        let do_remote = if yes {
            true
        } else if quiet {
            false
        } else {
            let prompt = "Some branches exist on remote. Also delete remote tracking branches?";
            tui::confirm(prompt, tui_title)?
        };

        if do_remote {
            for (name, _local, remotes) in &branch_info {
                for remote in remotes {
                    match git_ops::run_git(&["push", remote, "--delete", name]) {
                        Ok(_) => {
                            color::print_success(
                                &format!("Deleted remote ({remote}): {name}"),
                                None,
                            );
                            deleted_remote += 1;
                        }
                        Err(e) => {
                            color::print_error(
                                &format!("Failed to delete '{name}' on remote '{remote}': {e}"),
                                None,
                            );
                        }
                    }
                }
            }
        }
    }

    // Close PRs if requested via flag or if user agrees when prompted.
    if close_prs {
        close_prs_for_branches(&branches)?;
    } else if !quiet || yes {
        prompt_close_prs(&branches, yes, quiet, tui_title)?;
    }

    if deleted_local > 0 || deleted_remote > 0 {
        let msg = if deleted_remote > 0 {
            format!("\nDiscarded {deleted_local} local and {deleted_remote} remote branch(es).")
        } else {
            format!("\nDiscarded {deleted_local} branch(es).")
        };
        color::print_success(&msg, None);
    }

    Ok(())
}

fn resolve_workflow(
    integration: Option<&str>,
    quiet: bool,
    current_branch: &str,
    tui_title: &str,
) -> Result<String> {
    if let Some(input) = integration {
        let branch = if planner::parse_integration_branch(input).is_some() {
            input.to_string()
        } else {
            planner::integration_branch_name(current_branch, input)
        };
        if !git_ops::branch_exists_anywhere(&branch)? {
            bail!("integration branch '{branch}' not found");
        }
        return Ok(branch);
    }

    let all_local = git_ops::list_local_branches()?;
    let integrations: Vec<String> = all_local
        .into_iter()
        .filter(|b| planner::parse_integration_branch(b).is_some())
        .collect();

    if integrations.is_empty() {
        bail!("no Mergetopus integration branches found");
    }

    if quiet {
        bail!("multiple integration branches found; pass one explicitly in --quiet mode");
    }

    let picked = tui::pick_branch(&integrations, tui_title, None, &[])?;
    picked.ok_or_else(|| anyhow::anyhow!("no workflow selected"))
}

/// Check for open PRs and ask the user whether to close them.
fn prompt_close_prs(branches: &[String], yes: bool, quiet: bool, tui_title: &str) -> Result<()> {
    let (forge, repo_path) = match resolve_forge_and_repo() {
        Ok(pair) => pair,
        Err(_) => return Ok(()),
    };

    let mut open_found = false;
    let mut prs_to_close: Vec<(u64, String)> = Vec::new();

    for branch in branches {
        match forge.find_pr_by_head(&repo_path, branch) {
            Ok(Some(pr)) if pr.state == forges::PrState::Open => {
                prs_to_close.push((pr.number, branch.to_string()));
                open_found = true;
            }
            _ => {}
        }
    }

    if !open_found {
        return Ok(());
    }

    let do_close = if yes {
        true
    } else if quiet {
        false
    } else {
        let branch_list = prs_to_close
            .iter()
            .map(|(num, name)| format!("  PR #{num} for {name}"))
            .collect::<Vec<_>>()
            .join("\n");
        let prompt = format!("Open pull/merge requests found:\n{branch_list}\n\nClose these PRs?");
        tui::confirm(&prompt, tui_title)?
    };

    if do_close {
        color::print_emphasis("\nClosing pull/merge requests:", None);
        for (num, branch) in &prs_to_close {
            match forge.close_pr(&repo_path, *num) {
                Ok(_) => color::print_success(&format!("  Closed PR #{num} for {branch}"), None),
                Err(e) => color::print_error(
                    &format!("  Failed to close PR #{num} for {branch}: {e}"),
                    None,
                ),
            }
        }
    }

    Ok(())
}

/// Resolve a forge client and `owner/repo` path from the configured remotes.
/// Prefers `origin`, then tries every other remote until one yields a
/// parsable URL and a supported forge; never blindly picks the
/// alphabetically first remote.
pub(crate) fn resolve_forge_and_repo() -> Result<(Box<dyn forges::Forge>, String)> {
    let mut remotes = git_ops::list_remote_names()?;
    remotes.sort_by_key(|r| if r == "origin" { 0 } else { 1 });
    if remotes.is_empty() {
        bail!("no remotes configured");
    }

    let mut last_err = anyhow::anyhow!("no configured remote yielded a usable forge");
    for remote in remotes {
        let remote_url = match git_ops::get_remote_url(&remote) {
            Ok(u) => u,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        let info = match parse_remote_url(&remote_url) {
            Ok(i) => i,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        match detect_forge(&remote_url) {
            Ok(forge) => {
                let repo_path = format!("{}/{}", info.owner, info.repo);
                return Ok((forge, repo_path));
            }
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

fn close_prs_for_branches(branches: &[String]) -> Result<()> {
    let (forge, repo_path) = match resolve_forge_and_repo() {
        Ok(pair) => pair,
        Err(e) => {
            color::print_warning(&format!("  (skipping PR close: {e})"), None);
            return Ok(());
        }
    };

    color::print_emphasis("\nClosing pull/merge requests:", None);
    for branch in branches {
        match forge.find_pr_by_head(&repo_path, branch) {
            Ok(Some(pr)) => {
                if pr.state == forges::PrState::Open {
                    match forge.close_pr(&repo_path, pr.number) {
                        Ok(_) => color::print_success(
                            &format!("  Closed PR #{pr} for {branch}", pr = pr.number),
                            None,
                        ),
                        Err(e) => color::print_error(
                            &format!(
                                "  Failed to close PR #{pr} for {branch}: {e}",
                                pr = pr.number
                            ),
                            None,
                        ),
                    }
                } else {
                    color::print_info(
                        &format!(
                            "  Skipping PR #{pr} for {branch} (state: {state})",
                            pr = pr.number,
                            state = pr.state
                        ),
                        None,
                    );
                }
            }
            Ok(None) => {
                color::print_info(&format!("  No open PR found for {branch}"), None);
            }
            Err(e) => {
                color::print_warning(&format!("  Error looking up PR for {branch}: {e}"), None);
            }
        }
    }

    Ok(())
}
