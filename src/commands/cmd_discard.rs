use anyhow::{Result, bail};

use crate::color;
use crate::forges;
use crate::forges::detect::{detect_forge, parse_remote_url};
use crate::git_ops;
use crate::planner;
use crate::tui;

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

    // Determine which branches exist locally and/or on remote tracking refs.
    let mut branch_info: Vec<(String, bool, bool)> = Vec::new();
    for branch in &branches {
        let local = git_ops::branch_exists(branch)?;
        let remote_refs = git_ops::remote_refs_for_local_branch(branch)?;
        let on_remote = !remote_refs.is_empty();
        branch_info.push((branch.clone(), local, on_remote));
    }

    if branch_info.iter().all(|(_, local, on_remote)| !local && !on_remote) {
        color::print_warning(
            &format!("No branches found for workflow '{integration_branch}'"),
            None,
        );
        return Ok(());
    }

    let display: Vec<String> = branch_info
        .iter()
        .map(|(name, local, on_remote)| {
            let mut tags = Vec::new();
            if *local {
                tags.push("local");
            }
            if *on_remote {
                tags.push("remote");
            }
            format!("{name}  ({})", tags.join(", "))
        })
        .collect();

    if quiet && !yes {
        bail!("discard requires interactive confirmation; re-run without --quiet to proceed, or use --yes to auto-confirm");
    }

    let confirmed = yes || tui::confirm_list(
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

    for (name, local, _on_remote) in &branch_info {
        if *local {
            if name == current_branch {
                color::print_error(
                    &format!("Skipping '{name}': cannot delete the currently checked-out branch"),
                    None,
                );
                continue;
            }
            git_ops::delete_branch(name)?;
            color::print_success(&format!("Deleted local: {name}"), None);
            deleted_local += 1;
        }
    }

    let remote_names = git_ops::list_remote_names()?;
    let has_remote_branches = branch_info.iter().any(|(_, _, on_remote)| *on_remote);

    if has_remote_branches && !remote_names.is_empty() {
        let do_remote = if yes {
            true
        } else if quiet {
            false
        } else {
            let prompt = "Some branches exist on remote. Also delete remote tracking branches?";
            tui::confirm(prompt, tui_title)?
        };

        if do_remote {
            let remote = match remote_names.first() {
                Some(r) => r.clone(),
                None => {
                    color::print_warning("  No remotes configured.", None);
                    return Ok(());
                }
            };

            for (name, _local, on_remote) in &branch_info {
                if *on_remote {
                    match git_ops::run_git(&["push", "--delete", &remote, name]) {
                        Ok(_) => {
                            color::print_success(&format!("Deleted remote ({remote}): {name}"), None);
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
fn prompt_close_prs(
    branches: &[String],
    yes: bool,
    quiet: bool,
    tui_title: &str,
) -> Result<()> {
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
        let prompt = format!(
            "Open pull/merge requests found:\n{branch_list}\n\nClose these PRs?"
        );
        tui::confirm(&prompt, tui_title)?
    };

    if do_close {
        color::print_emphasis("\nClosing pull/merge requests:", None);
        for (num, branch) in &prs_to_close {
            match forge.close_pr(&repo_path, *num) {
                Ok(_) => color::print_success(
                    &format!("  Closed PR #{num} for {branch}"), None,
                ),
                Err(e) => color::print_error(
                    &format!("  Failed to close PR #{num} for {branch}: {e}"), None,
                ),
            }
        }
    }

    Ok(())
}

fn resolve_forge_and_repo() -> Result<(Box<dyn forges::Forge>, String)> {
    let remotes = git_ops::list_remote_names()?;
    let remote = remotes.first().ok_or_else(|| anyhow::anyhow!("no remotes configured"))?;
    let remote_url = git_ops::get_remote_url(remote)?;
    let forge = detect_forge(&remote_url)?;
    let info = parse_remote_url(&remote_url)?;
    let repo_path = format!("{}/{}", info.owner, info.repo);
    Ok((forge, repo_path))
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
                            &format!("  Closed PR #{pr} for {branch}", pr = pr.number), None,
                        ),
                        Err(e) => color::print_error(
                            &format!("  Failed to close PR #{pr} for {branch}: {e}", pr = pr.number), None,
                        ),
                    }
                } else {
                    color::print_info(
                        &format!("  Skipping PR #{pr} for {branch} (state: {state})", pr = pr.number, state = pr.state), None,
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
