use crate::tui;
use crate::color;
use anyhow::{Result, bail};

use crate::git_ops;
use crate::planner;

/// Removes resolved Mergetopus integration/slice branches that already have a
/// corresponding kokomeco branch, after interactive confirmation.
pub fn cleanup_command(quiet: bool, current_branch: &str, tui_title: &str) -> Result<()> {
    let all_local = git_ops::list_local_branches()?;

    let mut branches_to_delete: Vec<String> = Vec::new();

    for branch in &all_local {
        if planner::parse_integration_branch(branch).is_none() {
            continue;
        }

        let kokomeco = git_ops::consolidated_branch_name(branch);
        if !git_ops::branch_exists(&kokomeco)? {
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
