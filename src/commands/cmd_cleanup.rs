use crate::tui;
use anyhow::{Result, bail};
use mergetopus::color;
use mergetopus::forges;

use mergetopus::git_ops;
use mergetopus::planner;

/// Pure selection over a preloaded ref inventory: the local branches eligible
/// for cleanup — integration branches whose consolidated kokomeco branch
/// exists locally **or** on any remote-tracking ref — plus their locally
/// present slices. Kokomeco branches themselves are never returned (they are
/// retained), and remote-only branches are never candidates because cleanup
/// deletes local branches only.
///
/// Preloading matters: the original loop spent 1-2 subprocesses per branch
/// (and per slice on Separate-policy runs). Results are sorted and deduped.
fn cleanup_candidates(all_local: &[String], remote_refs: &[String]) -> Vec<String> {
    let local_set: std::collections::BTreeSet<&str> =
        all_local.iter().map(String::as_str).collect();
    let all_ref_names: Vec<&str> = all_local
        .iter()
        .map(String::as_str)
        .chain(remote_refs.iter().map(String::as_str))
        .collect();

    let mut candidates: Vec<String> = Vec::new();
    for branch in all_local {
        if planner::parse_integration_branch(branch).is_none() {
            continue;
        }

        let kokomeco = git_ops::consolidated_branch_name(branch);
        let suffix = format!("/{kokomeco}");
        let kokomeco_present = local_set.contains(kokomeco.as_str())
            || remote_refs.iter().any(|r| r.ends_with(&suffix));
        if !kokomeco_present {
            continue;
        }

        candidates.push(branch.clone());

        let slices = git_ops::slice_branch_names_from_refs(all_ref_names.iter().copied(), branch);
        for slice in slices {
            if local_set.contains(slice.as_str()) {
                candidates.push(slice);
            }
        }
    }

    candidates.sort();
    candidates.dedup();
    candidates
}

/// Removes resolved Mergetopus integration/slice branches that already have a
/// corresponding kokomeco branch, after interactive confirmation.
pub fn cleanup_command(
    close_prs: bool,
    quiet: bool,
    current_branch: &str,
    tui_title: &str,
) -> Result<()> {
    // Preload the ref inventory once; see `cleanup_candidates`.
    let all_local = git_ops::list_local_branches()?;
    let remote_refs = git_ops::list_remote_refs()?;

    let branches_to_delete = cleanup_candidates(&all_local, &remote_refs);

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
    // One linked-worktree probe for the whole deletion batch.
    let worktree_state = git_ops::WorktreeState::load()?;
    for branch in &branches_to_delete {
        if branch == current_branch {
            color::print_error(
                &format!("Skipping '{branch}': cannot delete the currently checked-out branch."),
                None,
            );
            continue;
        }
        git_ops::delete_branch_with(&worktree_state, branch)?;
        color::print_success(&format!("Deleted: {branch}"), None);
        deleted += 1;
    }

    color::print_success(&format!("\nCleaned up {deleted} branch(es)."), None);
    Ok(())
}

fn close_prs_for_branches(branches: &[String]) -> Result<()> {
    let (forge, repo_path) = match super::cmd_discard::resolve_forge_and_repo() {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn remote_only_kokomeco_selects_integration_and_local_slices() {
        // Regression for limitation #4: cleanup must recognize a kokomeco
        // branch that exists only as a remote-tracking ref.
        let all_local = names(&[
            "main",
            "_mmm/main/feature/integration",
            "_mmm/main/feature/slice1",
            "_mmm/main/feature/slice2",
        ]);
        let remote = names(&["origin/_mmm/main/feature/kokomeco"]);

        let got = cleanup_candidates(&all_local, &remote);

        assert_eq!(
            got,
            names(&[
                "_mmm/main/feature/integration",
                "_mmm/main/feature/slice1",
                "_mmm/main/feature/slice2",
            ])
        );
    }

    #[test]
    fn local_kokomeco_selects_and_kokomeco_is_retained() {
        let all_local = names(&[
            "_mmm/main/feature/integration",
            "_mmm/main/feature/slice1",
            "_mmm/main/feature/kokomeco",
        ]);

        let got = cleanup_candidates(&all_local, &[]);

        assert_eq!(
            got,
            names(&["_mmm/main/feature/integration", "_mmm/main/feature/slice1"])
        );
        assert!(
            !got.iter().any(|b| b.ends_with("/kokomeco")),
            "kokomeco branch must never be a cleanup candidate: {got:?}"
        );
    }

    #[test]
    fn no_kokomeco_anywhere_selects_nothing() {
        let all_local = names(&["_mmm/main/feature/integration", "_mmm/main/feature/slice1"]);

        assert!(cleanup_candidates(&all_local, &[]).is_empty());
        assert!(
            cleanup_candidates(
                &all_local,
                &names(&["origin/_mmm/main/feature/integration"])
            )
            .is_empty()
        );
    }

    #[test]
    fn remote_only_branches_are_not_candidates() {
        // Only local branches are deleted; a remote-only slice must not
        // appear even when its integration qualifies.
        let all_local = names(&["_mmm/main/feature/integration"]);
        let remote = names(&[
            "origin/_mmm/main/feature/kokomeco",
            "origin/_mmm/main/feature/slice1",
        ]);

        assert_eq!(
            cleanup_candidates(&all_local, &remote),
            names(&["_mmm/main/feature/integration"])
        );
    }

    #[test]
    fn remote_integration_only_is_not_a_candidate() {
        // With no local integration branch there is nothing local to delete.
        let remote = names(&[
            "origin/_mmm/main/feature/integration",
            "origin/_mmm/main/feature/kokomeco",
        ]);

        assert!(cleanup_candidates(&[], &remote).is_empty());
    }
}
