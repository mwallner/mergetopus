use crate::cli::Args;
use crate::color;
use crate::commands::cmd_merge_workflow;
use crate::models::SlicePlanItem;
use crate::tui;
use crate::tui_progress;
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};

use crate::git_ops;
use crate::planner;

/// Converts an in-progress manual merge into a Mergetopus-managed integration
/// flow by preserving resolved work, slicing unresolved conflicts, and creating
/// integration/slice branches for subsequent resolve steps.
///
/// `source_ref_override` — when provided (via `--source`), skip ref resolution
/// and use this value as the source label directly.
pub fn here_command(
    args: &Args,
    source_ref_override: Option<&str>,
    current_branch: &str,
    tui_title: &str,
) -> Result<()> {
    if !git_ops::merge_in_progress()? {
        bail!("HERE requires an in-progress merge (MERGE_HEAD not found)");
    }

    let source_sha = git_ops::merge_head_sha()?;
    let source_ref = match source_ref_override {
        Some(val) => val.to_string(),
        None => choose_source_ref_label(&source_sha, current_branch, args.quiet, tui_title)?,
    };
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
        color::print_info("No unresolved conflicts found in current merge. Nothing to slice.", None);
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

    if !args.quiet {
        let ib = integration_branch.clone();
        let rh = remembered_head.clone();
        let ss = source_sha.clone();
        tui_progress::run_progress(
            tui_title,
            vec![
                tui_progress::ProgressStep {
                    label: "Creating integration branch".into(),
                    action: Box::new(move || git_ops::checkout_new_or_reset(&ib, &rh)),
                },
                tui_progress::ProgressStep {
                    label: format!("Merging source: {source_ref}"),
                    action: Box::new(move || git_ops::merge_no_commit(&ss)),
                },
            ],
        )?;
    } else {
        git_ops::checkout_new_or_reset(&integration_branch, &remembered_head)?;
        git_ops::merge_no_commit(&source_sha)?;
    }

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
                color::print_error(&format!(
                    "Warning: failed to checkout '{current_branch}' during HERE cleanup: {checkout_err}"
                ), None);
            }
            if let Err(delete_err) = git_ops::delete_branch(&integration_branch) {
                color::print_error(&format!(
                    "Warning: failed to delete integration branch '{}' during HERE cleanup: {}",
                    integration_branch, delete_err
                ), None);
            }
            return Err(e)
                .context("conflict selection canceled during HERE; integration branch cleaned up");
        }
    };

    if !args.quiet {
        let ib = integration_branch.clone();
        let ib2 = integration_branch.clone();
        let mb = merge_base.clone();
        let sr = source_ref.clone();
        let ss = source_sha.clone();
        let ub = unresolved_before.clone();
        let es = explicit_slices.clone();
        tui_progress::run_progress(
            tui_title,
            vec![
                tui_progress::ProgressStep {
                    label: "Creating slice branches".into(),
                    action: Box::new(move || {
                        planner::create_slice_branches(&ib, &mb, &sr, &ss, &ub, &es)
                    }),
                },
                tui_progress::ProgressStep {
                    label: "Finalizing".into(),
                    action: Box::new(move || git_ops::checkout(&ib2)),
                },
            ],
        )?;
    } else {
        planner::create_slice_branches(
            &integration_branch,
            &merge_base,
            &source_ref,
            &source_sha,
            &unresolved_before,
            &explicit_slices,
        )?;
        git_ops::checkout(&integration_branch)?;
    }
    color::print_emphasis("Mergetopus HERE takeover complete", None);
    color::print_info(&format!("  Integration branch: {integration_branch}"), None);
    color::print_info(&format!("  Source ref: {source_ref} ({source_sha})"), None);
    color::print_info(&format!("  Remaining conflict count: {}", unresolved_before.len()), None);
    color::print_info(&format!("  Explicit slice groups: {}", explicit_slices.len()), None);

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

/// Resolve a human-readable source ref label for the given commit.
///
/// When exactly one local branch points to the commit, it is used directly.
/// When multiple local branches point to the same commit (e.g. after a
/// fast-forward merge), the user is prompted to pick one in interactive mode,
/// or a heuristic is applied in quiet mode (current branch excluded, MMM
/// branches excluded); if ambiguity remains, an error is raised suggesting
/// `--source`.
fn choose_source_ref_label(
    source_sha: &str,
    current_branch: &str,
    quiet: bool,
    tui_title: &str,
) -> Result<String> {
    let refs = git_ops::refs_pointing_to(source_sha)?;
    let local_refs: Vec<&String> = refs.iter().filter(|r| !r.contains('/')).collect();

    // Single candidate — straightforward.
    if let Some(single) = local_refs.iter().find(|_| local_refs.len() == 1) {
        return Ok((*single).clone());
    }

    // No local refs at all — fall back to remote or abbreviated SHA.
    if local_refs.is_empty() {
        if let Some(any) = refs.first() {
            return Ok(any.clone());
        }
        return Ok(source_sha[..8.min(source_sha.len())].to_string());
    }

    // Multiple local refs — disambiguate.
    if quiet {
        let filtered: Vec<&String> = local_refs
            .iter()
            .filter(|r| ***r != current_branch)
            .filter(|r| !r.starts_with("_mmm/"))
            .copied()
            .collect();

        match filtered.len() {
            0 => {
                // All candidates were filtered out — still ambiguous, just
                // show everything except current branch.
                let candidates: Vec<&String> = local_refs
                    .iter()
                    .filter(|r| ***r != current_branch)
                    .copied()
                    .collect();
                if candidates.len() == 1 {
                    return Ok(candidates[0].clone());
                }
                bail!(
                    "ambiguous source ref '{:.8}'; candidates: {}. Use --source to specify.",
                    source_sha,
                    candidates.iter().map(|r| r.as_str()).collect::<Vec<_>>().join(", ")
                );
            }
            1 => Ok(filtered[0].clone()),
            _ => {
                bail!(
                    "ambiguous source ref '{:.8}'; candidates: {}. Use --source to specify.",
                    source_sha,
                    filtered.iter().map(|r| r.as_str()).collect::<Vec<_>>().join(", ")
                );
            }
        }
    } else {
        let candidates: Vec<String> = local_refs.iter().map(|r| (*r).clone()).collect();
        match tui::pick_branch(
            &candidates,
            tui_title,
            Some(current_branch),
            &[],
        )? {
            Some(choice) => Ok(choice),
            None => bail!("source ref selection canceled"),
        }
    }
}
