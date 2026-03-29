use anyhow::{Result, bail};

use crate::git_ops;
use crate::helpers;
use crate::planner;
use crate::tui;

use helpers::extract_slice_paths;

/// Reports Mergetopus integration progress for a source/integration branch,
/// including slice merge state and suggested next commands.
pub fn status_command(
    source_arg: Option<&str>,
    quiet: bool,
    current_branch: &str,
    tui_title: &str,
) -> Result<()> {
    let integration_branch =
        resolve_status_integration_branch(source_arg, quiet, current_branch, tui_title)?;

    // If a kokomeco consolidated branch already exists for this integration
    // branch, show the merge suggestion instead of the raw integration status.
    let kokomeco = git_ops::consolidated_branch_name(&integration_branch);
    if git_ops::branch_exists(&kokomeco)? {
        println!("Mergetopus status");
        println!("  Integration branch:  {integration_branch}");
        println!("  Consolidated branch: {kokomeco}");
        println!();
        println!(
            "All slices are resolved. The kokomeco branch is ready to merge into '{current_branch}'."
        );
        println!();
        println!("Suggested next command:");
        println!("  git merge {kokomeco}");
        println!();
        println!("To clean up slice and integration branches afterward:");
        println!("  mergetopus cleanup");
        return Ok(());
    }

    let initial_commit = git_ops::first_parent_oldest_commit(&integration_branch)?;
    let initial_message = git_ops::commit_message(&initial_commit)?;
    let parents = git_ops::commit_parent_shas(&initial_commit)?;

    let source_sha = parents
        .get(1)
        .cloned()
        .unwrap_or_else(|| "(unknown)".to_string());
    let source_ref =
        parse_partial_merge_source_ref(&initial_message).unwrap_or_else(|| "(unknown)".to_string());

    let slices = git_ops::list_slice_branches_for_integration(&integration_branch)?;
    let status = git_ops::slice_merge_status(&integration_branch, &slices)?;

    let merged = status.values().filter(|v| **v).count();
    let pending = status.values().filter(|v| !**v).count();

    println!("Mergetopus status");
    println!("  Integration branch: {integration_branch}");
    println!("  Source ref: {source_ref}");
    println!("  Source SHA: {source_sha}");
    println!("  Total slices: {}", slices.len());
    println!("  Merged slices: {merged}");
    println!("  Pending slices: {pending}");

    if pending > 0 {
        println!("\nPending slice details:");
        for slice in &slices {
            let is_merged = status.get(slice).copied().unwrap_or(false);
            if is_merged {
                continue;
            }

            let tip_msg = git_ops::branch_tip_commit_message(slice)?;
            let mut paths = extract_slice_paths(&tip_msg);
            let resolve_tip = tip_msg
                .lines()
                .next()
                .unwrap_or("")
                .contains("Mergetopus resolve:");
            if paths.is_empty() && resolve_tip {
                let parent = git_ops::parent_sha(slice)?;
                let parent_msg = git_ops::commit_message(&parent)?;
                paths = extract_slice_paths(&parent_msg);
            }

            println!(
                "  - {slice}: {}",
                if resolve_tip {
                    "resolved (not merged)"
                } else {
                    "pending resolution"
                }
            );

            if paths.is_empty() {
                println!("    detected paths: (unknown)");
            } else {
                println!("    detected paths: {}", paths.join(", "));
            }
        }
    }

    println!("\nSuggested next command(s):");
    if pending == 0 {
        if slices.is_empty() {
            println!("  - No slice branches were found for this integration branch.");
        } else if source_ref == "(unknown)" {
            println!("  - mergetopus <source> --yes");
        } else {
            println!("  - mergetopus {source_ref} --yes");
        }
    } else {
        let first_pending = slices
            .iter()
            .find(|s| !status.get(*s).copied().unwrap_or(false));
        if let Some(slice) = first_pending {
            println!("  - mergetopus resolve {slice}");
            println!("  - git checkout {integration_branch} && git merge --no-ff {slice}");
        }
    }

    Ok(())
}

fn parse_partial_merge_source_ref(message: &str) -> Option<String> {
    let first = message.lines().next()?.trim();
    let prefix = "Mergetopus: partial merge '";
    let rest = first.strip_prefix(prefix)?;
    let end = rest.find("' into '")?;
    let source = &rest[..end];
    if source.is_empty() {
        None
    } else {
        Some(source.to_string())
    }
}

fn resolve_status_integration_branch(
    source_arg: Option<&str>,
    quiet: bool,
    current_branch: &str,
    tui_title: &str,
) -> Result<String> {
    if let Some(source) = source_arg {
        let target = if planner::parse_integration_branch(source).is_some() {
            source.to_string()
        } else if let Some((_, current_source)) = planner::parse_integration_branch(current_branch)
        {
            if current_source == planner::sanitize_branch_fragment(source) {
                current_branch.to_string()
            } else {
                planner::integration_branch_name(current_branch, source)
            }
        } else {
            planner::integration_branch_name(current_branch, source)
        };

        if !git_ops::branch_exists(&target)? {
            bail!(
                "could not find integration branch '{}'; provide an existing integration branch or source ref",
                target
            );
        }

        return Ok(target);
    }

    if planner::parse_integration_branch(current_branch).is_some() {
        return Ok(current_branch.to_string());
    }

    let prefix = planner::integration_branch_family_prefix(current_branch);
    let mut candidates = git_ops::list_local_branches()?
        .into_iter()
        .filter(|b| b.starts_with(&prefix))
        .filter(|b| planner::parse_integration_branch(b).is_some())
        .collect::<Vec<_>>();

    match candidates.len() {
        0 => bail!(
            "no integration branches found for current branch '{}'; provide SOURCE explicitly, e.g. 'mergetopus status <source>'",
            current_branch
        ),
        1 => Ok(candidates.remove(0)),
        _ => {
            if quiet {
                bail!(
                    "multiple integration branches found; provide SOURCE explicitly in --quiet mode"
                );
            }
            match tui::pick_branch(&candidates, tui_title)? {
                Some(b) => Ok(b),
                None => bail!("status branch selection was canceled"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_partial_merge_source_ref_works() {
        let msg = "Mergetopus: partial merge 'origin/feature-x' into '_mmm/main/origin_feature-x/integration' (conflicts sliced)\n\nmerged:\n* a";
        assert_eq!(
            parse_partial_merge_source_ref(msg),
            Some("origin/feature-x".to_string())
        );
    }

    #[test]
    fn parse_partial_merge_source_ref_rejects_non_matching() {
        assert_eq!(parse_partial_merge_source_ref("hello"), None);
    }
}
