use std::collections::BTreeMap;

use crate::cli::Args;
use crate::models::SlicePlanItem;
use anyhow::{Context, Result, bail};

use crate::git_ops;
use crate::planner;
use crate::tui;

/// Runs the primary Mergetopus merge workflow: chooses/normalizes source,
/// creates or resumes integration context, snapshots auto-merged files, plans
/// conflict slices, and materializes slice branches for iterative resolution.
pub fn run_merge_workflow(args: &Args, current_branch: &str, tui_title: &str) -> Result<()> {
    let selected_source_ref = match args.effective_source() {
        Some(s) => s.to_string(),
        None => {
            if args.quiet {
                bail!("--quiet requires SOURCE to be provided explicitly");
            }
            let mut branches = git_ops::list_branch_refs()?;
            let remote_names = git_ops::list_remote_names()?;

            // Slice branches are resolve targets only; don't allow selecting them as a source.
            branches.retain(|b| !planner::is_slice_branch(b));

            match tui::pick_branch(&branches, tui_title, Some(current_branch), &remote_names)? {
                Some(b) => b,
                None => bail!("merge source selection was canceled"),
            }
        }
    };
    let source_ref = normalize_merge_source_ref(&selected_source_ref)?;

    // If an integration branch is selected, redirect to the original/source pair.
    let (actual_source_ref, actual_integration_branch, target_branch_for_merge) =
        if let Some((original, source)) = planner::parse_integration_branch(&source_ref) {
            println!("Note: '{source_ref}' is an integration branch.");
            println!("Redirecting: checking out '{original}' and merging '{source}' instead.\n");

            git_ops::checkout(&original)?;
            let new_current = git_ops::current_branch()?;
            let new_integration = planner::integration_branch_name(&new_current, &source);

            (source.clone(), new_integration, new_current)
        } else {
            let integration_branch = planner::integration_branch_name(current_branch, &source_ref);
            (
                source_ref.clone(),
                integration_branch,
                current_branch.to_string(),
            )
        };

    let kokomeco_branch = git_ops::consolidated_branch_name(&actual_integration_branch);
    if git_ops::branch_exists(&kokomeco_branch)? {
        println!("Kokomeco branch already exists for this merge context: {kokomeco_branch}");
        println!("To merge it back into your current target branch:");
        println!("  git checkout {target_branch_for_merge}");
        println!("  git merge --no-ff {kokomeco_branch}");
        println!("After promotion, delete it manually when no longer needed:");
        println!("  git branch -d {kokomeco_branch}");
        return Ok(());
    }

    let actual_source_sha = git_ops::resolve_commit(&actual_source_ref)?;
    let actual_remembered_head = git_ops::head_sha()?;
    let actual_merge_base = git_ops::merge_base(&actual_remembered_head, &actual_source_sha)
        .with_context(|| {
            "failed before entering conflict resolution: could not compute merge-base between current HEAD and source; verify source/history compatibility, then retry (for unrelated histories, merge manually with --allow-unrelated-histories first)"
        })?;

    if git_ops::branch_exists(&actual_integration_branch)? {
        git_ops::checkout(&actual_integration_branch)?;
        let slices = git_ops::list_slice_branches_for_integration(&actual_integration_branch)?;
        let status = git_ops::slice_merge_status(&actual_integration_branch, &slices)?;

        if !status.is_empty() {
            println!("Existing slice merge status for {actual_integration_branch}:");
            for (slice, merged) in &status {
                println!(
                    "  - {slice}: {}",
                    if *merged { "merged" } else { "pending" }
                );
            }
        }

        let has_slices = !status.is_empty();
        let all_merged = status.values().all(|v| *v);
        if all_merged {
            let do_consolidate = if args.yes {
                true
            } else if args.quiet {
                println!(
                    "No pending slice merges. Skipping kokomeco prompt due to --quiet (use --yes to auto-create the kokomeco branch)."
                );
                false
            } else {
                tui::confirm(
                    if has_slices {
                        "All slice branches are already merged. Create a non-destructive kokomeco merge commit branch? (Enter/y=yes, n/Esc=no)"
                    } else {
                        "No slice branches were created for this integration branch. Create a non-destructive kokomeco merge commit branch? (Enter/y=yes, n/Esc=no)"
                    },
                    tui_title,
                )?
            };

            if do_consolidate {
                let consolidated = create_consolidated_merge_commit_branch(
                    &actual_integration_branch,
                    &actual_source_ref,
                    &status,
                )?;
                println!("Created kokomeco branch: {consolidated}");
                println!(
                    "Integration branch was not rewritten. Review and promote explicitly if desired."
                );
            }
        }

        if !all_merged {
            println!("Integration branch already exists and has pending slice merges.");
            println!(
                "Resolve pending slices first, then re-run for consolidation or new operations."
            );
        }

        return Ok(());
    }

    git_ops::checkout_new_or_reset(&actual_integration_branch, &actual_remembered_head)?;
    git_ops::merge_no_commit(&actual_source_ref)?;

    let conflicted_files = git_ops::conflicted_files()?;
    for path in &conflicted_files {
        git_ops::restore_ours(path)?;
    }

    let auto_merged_files = git_ops::staged_files()?;

    let slice_plan = conflicted_files
        .iter()
        .enumerate()
        .map(|(i, file)| {
            let branch = planner::slice_branch_name(&actual_integration_branch, i + 1)?;
            Ok(SlicePlanItem {
                path: file.clone(),
                branch,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    if git_ops::merge_in_progress()? {
        let merged_section = if auto_merged_files.is_empty() {
            "* (none)".to_string()
        } else {
            auto_merged_files
                .iter()
                .map(|f| format!("* {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let sliced_section = if slice_plan.is_empty() {
            "* (none)".to_string()
        } else {
            slice_plan
                .iter()
                .map(|s| format!("* {} -> {}", s.path, s.branch))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let msg = format!(
            "Mergetopus: partial merge '{actual_source_ref}' into '{actual_integration_branch}' (conflicts sliced)\n\nmerged:\n{merged_section}\n\nsliced:\n{sliced_section}"
        );

        git_ops::commit(&msg)?;
    }

    let explicit_slices = match select_conflicts(
        args,
        &actual_source_ref,
        &conflicted_files,
        tui_title,
    ) {
        Ok(slices) => slices,
        Err(e) => {
            // Clean up on cancellation: checkout target branch and delete integration branch.
            if let Err(checkout_err) = git_ops::checkout(&target_branch_for_merge) {
                eprintln!(
                    "Warning: failed to checkout '{target_branch_for_merge}' during cleanup: {checkout_err}"
                );
            }
            if let Err(delete_err) = git_ops::delete_branch(&actual_integration_branch) {
                eprintln!(
                    "Warning: failed to delete integration branch '{actual_integration_branch}' during cleanup: {delete_err}"
                );
            }
            return Err(e).context("conflict selection canceled; integration branch cleaned up");
        }
    };
    planner::create_slice_branches(
        &actual_integration_branch,
        &actual_merge_base,
        &actual_source_ref,
        &actual_source_sha,
        &conflicted_files,
        &explicit_slices,
    )?;

    git_ops::checkout(&actual_integration_branch)?;
    println!("Mergetopus complete");
    println!("  Integration branch: {actual_integration_branch}");
    println!("  Source ref: {actual_source_ref} ({actual_source_sha})");
    println!("  Conflict count: {}", conflicted_files.len());
    println!("  Explicit slice groups: {}", explicit_slices.len());
    for (idx, group) in explicit_slices.iter().enumerate() {
        println!("  - SliceGroup {}: {} file(s)", idx + 1, group.len());
    }

    Ok(())
}

pub fn select_conflicts(
    args: &Args,
    source_ref: &str,
    all_conflicts: &[String],
    tui_title: &str,
) -> Result<Vec<Vec<String>>> {
    match args.select_paths.as_deref() {
        Some(csv) => {
            let paths = git_ops::select_conflicts_by_list(all_conflicts, csv)?;
            if paths.is_empty() {
                Ok(Vec::new())
            } else {
                Ok(vec![paths])
            }
        }
        None => {
            if args.quiet {
                Ok(Vec::new())
            } else {
                let diff_tool = git_ops::get_git_config("diff.tool")?;
                match tui::select_conflicts(
                    all_conflicts,
                    |path| git_ops::three_way_diff(path, source_ref),
                    diff_tool.as_deref(),
                    |path| git_ops::launch_difftool(path, source_ref),
                    tui_title,
                )? {
                    Some(groups) => Ok(groups),
                    None => bail!("conflict selection canceled"),
                }
            }
        }
    }
}

fn normalize_merge_source_ref(source_ref: &str) -> Result<String> {
    let trimmed = source_ref.trim_start_matches('/');

    if !git_ops::remote_branch_exists(trimmed)? {
        return Ok(trimmed.to_string());
    }

    let Some((remote_name, local_candidate)) = trimmed.split_once('/') else {
        return Ok(trimmed.to_string());
    };

    if local_candidate.is_empty() {
        bail!(
            "selected source '{}' is a remote ref with no local branch name; choose a concrete branch",
            source_ref
        );
    }

    if git_ops::branch_exists(local_candidate)? {
        // Local branch exists: check if it's in sync with the remote.
        let local_sha = git_ops::resolve_ref(local_candidate)?;
        let remote_sha = git_ops::resolve_ref(trimmed)?;

        if local_sha == remote_sha {
            // Local is in sync with remote; use it.
            println!("Using existing local branch '{local_candidate}' (in sync with '{trimmed}').");
            return Ok(local_candidate.to_string());
        } else {
            // Local and remote diverge.
            bail!(
                "selected source '{}' maps to local branch '{}' which has diverged from its remote counterpart; \
                 local is at {} while remote is at {}. Synchronize or use a different branch.",
                source_ref,
                local_candidate,
                &local_sha[..8.min(local_sha.len())],
                &remote_sha[..8.min(remote_sha.len())]
            );
        }
    }

    git_ops::create_tracking_branch(local_candidate, trimmed)?;
    println!("Using remote source '{trimmed}' via new local tracking branch '{local_candidate}'");
    println!(
        "Tip: remote name '{remote_name}' is omitted for this merge context (source = '{local_candidate}')."
    );

    Ok(local_candidate.to_string())
}

fn create_consolidated_merge_commit_branch(
    integration_branch: &str,
    source_ref: &str,
    slice_merge_status: &BTreeMap<String, bool>,
) -> Result<String> {
    // Derive remembered head and source commit from the first mergetopus
    // partial-merge commit on the integration branch, not from the oldest
    // reachable ancestor in the repository history.
    let initial_commit = git_ops::first_mergetopus_partial_merge_commit(integration_branch)?;

    let remembered_head =
        git_ops::run_git(&["rev-parse", "--verify", &format!("{initial_commit}^1")])
            .context("failed to resolve remembered head from initial integration commit")?;
    let source_sha = git_ops::run_git(&["rev-parse", "--verify", &format!("{initial_commit}^2")])
        .context(
        "failed to resolve source SHA from initial integration commit (expected a merge commit)",
    )?;

    let merged_slices = slice_merge_status
        .iter()
        .filter(|(_, merged)| **merged)
        .map(|(name, _)| format!("* {name}"))
        .collect::<Vec<_>>()
        .join("\n");

    let message = format!(
        "Mergetopus consolidated merge: '{source_ref}' into '{integration_branch}'\n\nThis commit snapshots the resolved integration tree into one merge commit.\n\nSource-Ref: {source_ref}\nSource-Commit: {source_sha}\nRemembered-Head: {remembered_head}\nMerged-Slices:\n{}",
        if merged_slices.is_empty() {
            "* (none)"
        } else {
            &merged_slices
        }
    );

    let branch = git_ops::consolidated_branch_name(integration_branch);
    git_ops::checkout_new_or_reset(&branch, &remembered_head)?;

    // Start the merge to establish the correct merge parents, then replace the
    // index/worktree content with the final integration tree before committing.
    git_ops::merge_no_commit(&source_sha)?;

    // Replace staged/worktree content with the resolved integration branch content.
    git_ops::run_git(&[
        "restore",
        &format!("--source={integration_branch}"),
        "--staged",
        "--worktree",
        "--",
        ".",
    ])
    .context("failed to overlay integration branch content onto consolidated branch")?;

    git_ops::commit(&message)?;
    Ok(branch)
}
