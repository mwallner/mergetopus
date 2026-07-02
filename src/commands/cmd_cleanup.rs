use crate::tui;
use crate::color;
use crate::forges;
use crate::forges::detect::{detect_forge, parse_remote_url};
use anyhow::{Result, bail};

use crate::git_ops;
use crate::planner;

/// Removes resolved Mergetopus integration/slice branches that already have a
/// corresponding kokomeco branch, after interactive confirmation.
pub fn cleanup_command(close_prs: bool, quiet: bool, current_branch: &str, tui_title: &str) -> Result<()> {
    let all_local = git_ops::list_local_branches()?;

    let mut branches_to_delete: Vec<String> = Vec::new();

    for branch in &all_local {
        if planner::parse_integration_branch(branch).is_none() {
            continue;
        }

        let kokomeco = git_ops::consolidated_branch_name(branch);
        if !git_ops::branch_exists_anywhere(&kokomeco)? {
            continue;
        }

        branches_to_delete.push(branch.clone());

        let slices = git_ops::list_slice_branches_for_integration(branch)?;
        for slice in slices {
            if git_ops::branch_exists(&slice)? {
                branches_to_delete.push(slice);
            }
        }
    }

    if branches_to_delete.is_empty() {
        color::print_info(
            "Nothing to clean up: no integration branches with a corresponding kokomeco branch found.",
            None,
        );
        color::print_info(
            "  Tip: run 'git fetch --prune' to sync remote state and try again.",
            None,
        );
        return Ok(());
    }

    branches_to_delete.sort();
    branches_to_delete.dedup();

    let do_delete = if quiet {
        bail!("cleanup requires interactive confirmation; re-run without --quiet to proceed");
    } else {
        tui::confirm_list(
            &branches_to_delete,
            &format!(
                "Delete {} branch(es)? The kokomeco branch is retained. This cannot be undone.",
                branches_to_delete.len()
            ),
            tui_title,
        )?
    };

    if !do_delete {
        color::print_warning("Cleanup canceled.", None);
        return Ok(());
    }

    if close_prs {
        close_prs_for_branches(&branches_to_delete)?;
    }

    let mut deleted = 0usize;
    for branch in &branches_to_delete {
        if branch == current_branch {
            color::print_error(&format!("Skipping '{branch}': cannot delete the currently checked-out branch."), None);
            continue;
        }
        git_ops::delete_branch(branch)?;
        color::print_success(&format!("Deleted: {branch}"), None);
        deleted += 1;
    }

    color::print_success(&format!("\nCleaned up {deleted} branch(es)."), None);
    Ok(())
}

fn close_prs_for_branches(branches: &[String]) -> Result<()> {
    let remotes = git_ops::list_remote_names()?;
    let remote = match remotes.first() {
        Some(r) => r.clone(),
        None => {
            color::print_warning("  No remotes configured; skipping PR close.", None);
            return Ok(());
        }
    };

    let remote_url = match git_ops::get_remote_url(&remote) {
        Ok(url) => url,
        Err(e) => {
            color::print_warning(&format!("  Could not read remote URL: {e}"), None);
            return Ok(());
        }
    };

    let forge = match detect_forge(&remote_url) {
        Ok(f) => f,
        Err(e) => {
            color::print_warning(&format!("  (skipping PR close: {e})",), None);
            return Ok(());
        }
    };

    let info = match parse_remote_url(&remote_url) {
        Ok(i) => i,
        Err(e) => {
            color::print_warning(&format!("  Could not parse remote URL: {e}"), None);
            return Ok(());
        }
    };
    let repo_path = format!("{}/{}", info.owner, info.repo);

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
