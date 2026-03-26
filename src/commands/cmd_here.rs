use crate::cli::Args;
use crate::commands::cmd_merge_workflow;
use crate::models::SlicePlanItem;
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};

use crate::git_ops;
use crate::planner;

pub(crate) fn here_command(args: &Args, current_branch: &str, tui_title: &str) -> Result<()> {
    if !git_ops::merge_in_progress()? {
        bail!("HERE requires an in-progress merge (MERGE_HEAD not found)");
    }

    let source_sha = git_ops::merge_head_sha()?;
    let source_ref = choose_source_ref_label(&source_sha)?;
    let integration_branch = planner::integration_branch_name(current_branch, &source_ref);
    let kokomeco_branch = git_ops::consolidated_branch_name(&integration_branch);

    if git_ops::branch_exists(&kokomeco_branch)? {
        bail!(
            "kokomeco branch already exists for this merge context: {}",
            kokomeco_branch
        );
    }
    if git_ops::branch_exists(&integration_branch)? {
        bail!(
            "integration branch '{}' already exists; use status/resolve or cleanup first",
            integration_branch
        );
    }

    let unresolved_before = git_ops::conflicted_files()?;
    if unresolved_before.is_empty() {
        println!("No unresolved conflicts found in current merge. Nothing to slice.");
        return Ok(());
    }

    // Preserve already-resolved merge work so takeover does not lose manual progress.
    let unresolved_set = unresolved_before.iter().cloned().collect::<BTreeSet<_>>();
    let mut resolved_paths = git_ops::staged_files()?;
    resolved_paths.extend(git_ops::unstaged_files()?);
    resolved_paths.retain(|p| !unresolved_set.contains(p));
    resolved_paths.sort();
    resolved_paths.dedup();
    let resolved_snapshots = snapshot_resolved_paths(&resolved_paths)?;

    git_ops::merge_abort()?;

    let remembered_head = git_ops::head_sha()?;
    let merge_base = git_ops::merge_base(&remembered_head, &source_sha)?;

    git_ops::checkout_new_or_reset(&integration_branch, &remembered_head)?;
    git_ops::merge_no_commit(&source_sha)?;

    let conflicted_now = git_ops::conflicted_files()?;
    for path in &conflicted_now {
        git_ops::restore_ours(path)?;
    }

    apply_resolved_snapshots(&resolved_snapshots)?;

    let auto_merged_files = git_ops::staged_files()?;
    let slice_plan = unresolved_before
        .iter()
        .enumerate()
        .map(|(i, file)| {
            let branch = planner::slice_branch_name(&integration_branch, i + 1)?;
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
            "Mergetopus: partial merge '{source_ref}' into '{integration_branch}' (conflicts sliced)\n\nmerged:\n{merged_section}\n\nsliced:\n{sliced_section}\n\nTakeover: HERE"
        );
        git_ops::commit(&msg)?;
    }

    let explicit_slices = match cmd_merge_workflow::select_conflicts(
        args,
        &source_ref,
        &unresolved_before,
        tui_title,
    ) {
        Ok(slices) => slices,
        Err(e) => {
            if let Err(checkout_err) = git_ops::checkout(current_branch) {
                eprintln!(
                    "Warning: failed to checkout '{current_branch}' during HERE cleanup: {checkout_err}"
                );
            }
            if let Err(delete_err) = git_ops::delete_branch(&integration_branch) {
                eprintln!(
                    "Warning: failed to delete integration branch '{}' during HERE cleanup: {}",
                    integration_branch, delete_err
                );
            }
            return Err(e)
                .context("conflict selection canceled during HERE; integration branch cleaned up");
        }
    };

    planner::create_slice_branches(
        &integration_branch,
        &merge_base,
        &source_ref,
        &source_sha,
        &unresolved_before,
        &explicit_slices,
    )?;

    git_ops::checkout(&integration_branch)?;
    println!("Mergetopus HERE takeover complete");
    println!("  Integration branch: {integration_branch}");
    println!("  Source ref: {source_ref} ({source_sha})");
    println!("  Remaining conflict count: {}", unresolved_before.len());
    println!("  Explicit slice groups: {}", explicit_slices.len());

    Ok(())
}

fn snapshot_resolved_paths(paths: &[String]) -> Result<BTreeMap<String, Option<Vec<u8>>>> {
    let mut snapshots = BTreeMap::new();
    for path in paths {
        let content = std::fs::read(path).ok();
        snapshots.insert(path.clone(), content);
    }
    Ok(snapshots)
}

fn apply_resolved_snapshots(snapshots: &BTreeMap<String, Option<Vec<u8>>>) -> Result<()> {
    for (path, content) in snapshots {
        match content {
            Some(bytes) => {
                if let Some(parent) = std::path::Path::new(path).parent()
                    && !parent.as_os_str().is_empty()
                {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("failed to create parent directory for '{path}'")
                    })?;
                }
                std::fs::write(path, bytes)
                    .with_context(|| format!("failed to restore resolved file '{path}'"))?;
                git_ops::stage_path(path)?;
            }
            None => {
                git_ops::rm_path(path)?;
            }
        }
    }
    Ok(())
}

fn choose_source_ref_label(source_sha: &str) -> Result<String> {
    let refs = git_ops::refs_pointing_to(source_sha)?;
    if let Some(local) = refs.iter().find(|r| !r.contains('/')) {
        return Ok(local.clone());
    }
    if let Some(any) = refs.first() {
        return Ok(any.clone());
    }
    Ok(source_sha[..8.min(source_sha.len())].to_string())
}
