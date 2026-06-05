use anyhow::{Result, bail};
use std::collections::BTreeMap;
use crate::color;

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
    if let Some(source) = source_arg {
        let integration_branch = resolve_status_integration_branch(source, current_branch)?;
        return print_integration_status(&integration_branch, quiet, current_branch, tui_title);
    }

    let discovered = discover_global_mmm_units()?;
    if discovered.units.is_empty() {
        color::print_info(
            "No in-progress Mergetopus merges found in '_mmm/' across local and configured remotes.",
            None,
        );
        if !discovered.orphaned_refs.is_empty() {
            print_orphaned_refs(&discovered.orphaned_refs);
        }
        return Ok(());
    }

    color::print_emphasis(&format!(
        "In-progress MMM merges detected: {}",
        discovered.units.len()
    ), None);
    println!();
    print_global_overview(&discovered.units);
    if !discovered.orphaned_refs.is_empty() {
        print_orphaned_refs(&discovered.orphaned_refs);
    }

    if let Some(current_unit) = select_current_branch_unit(&discovered.units, current_branch) {
        color::print_emphasis("\nCurrent branch details:", None);
        print_integration_status(&current_unit.integration, true, current_branch, tui_title)?;
    }

    Ok(())
}

#[derive(Clone, Debug)]
struct StatusUnit {
    integration: String,
    target: String,
    source: String,
    pending: usize,
    resolved: usize,
    kokomeco_present: bool,
}

#[derive(Debug)]
struct GlobalDiscovery {
    units: Vec<StatusUnit>,
    orphaned_refs: Vec<String>,
}

fn discover_global_mmm_units() -> Result<GlobalDiscovery> {
    let refs = git_ops::list_branch_refs()?;
    let remote_names = git_ops::list_remote_names()?;
    let mut all_mmm_refs = std::collections::BTreeSet::new();
    let mut integration_names = std::collections::BTreeSet::new();

    for reference in refs {
        let canonical = canonicalize_branch_ref(&reference, &remote_names);
        if !canonical.starts_with("_mmm/") {
            continue;
        }
        if planner::parse_integration_branch(&canonical).is_some() {
            integration_names.insert(canonical.clone());
        }
        all_mmm_refs.insert(canonical);
    }

    let mut orphaned_refs = std::collections::BTreeSet::new();
    for reference in &all_mmm_refs {
        if integration_names.contains(reference) {
            continue;
        }

        let expected_integration = if let Some(integration) = planner::integration_from_slice_branch(reference) {
            Some(integration)
        } else {
            integration_from_kokomeco_branch(reference)
        };

        match expected_integration {
            Some(integration) if !integration_names.contains(&integration) => {
                orphaned_refs.insert(reference.clone());
            }
            Some(_) => {}
            None => {
                orphaned_refs.insert(reference.clone());
            }
        }
    }

    let mut units = Vec::new();
    for integration in integration_names {
        units.push(build_status_unit(&integration)?);
    }
    units.sort_by(|a, b| a.integration.cmp(&b.integration));

    Ok(GlobalDiscovery {
        units,
        orphaned_refs: orphaned_refs.into_iter().collect(),
    })
}

fn canonicalize_branch_ref(reference: &str, remote_names: &[String]) -> String {
    let Some((prefix, tail)) = reference.split_once('/') else {
        return reference.to_string();
    };

    if !tail.is_empty() && remote_names.iter().any(|r| r == prefix) {
        return tail.to_string();
    }

    reference.to_string()
}

fn build_status_unit(integration_branch: &str) -> Result<StatusUnit> {
    let (target, source) = planner::parse_integration_branch(integration_branch)
        .unwrap_or_else(|| ("(unknown)".to_string(), "(unknown)".to_string()));
    let integration_ref =
        git_ops::best_ref_for_local_branch(integration_branch)?.unwrap_or_else(|| integration_branch.to_string());
    let slices = git_ops::list_slice_branches_for_integration(integration_branch)?;
    let status = slice_merge_status(&integration_ref, &slices)?;
    let resolved = status.values().filter(|v| **v).count();
    let pending = status.values().filter(|v| !**v).count();
    let kokomeco = git_ops::consolidated_branch_name(integration_branch);
    let kokomeco_present = git_ops::branch_exists_anywhere(&kokomeco)?;

    Ok(StatusUnit {
        integration: integration_branch.to_string(),
        target,
        source,
        pending,
        resolved,
        kokomeco_present,
    })
}

fn print_global_overview(units: &[StatusUnit]) {
    let mut integration_w = "Integration".len();
    let mut target_w = "Target".len();
    let mut source_w = "Source".len();

    for unit in units {
        integration_w = integration_w.max(unit.integration.len());
        target_w = target_w.max(unit.target.len());
        source_w = source_w.max(unit.source.len());
    }

    color::print_emphasis("Global MMM overview", None);
    color::print_info(&format!(
        "{:<integration_w$}  {:<target_w$}  {:<source_w$}  {:<8}  {:>7}  {:>8}  {:<8}",
        "Integration",
        "Target",
        "Source",
        "State",
        "Pending",
        "Resolved",
        "Kokomeco",
        integration_w = integration_w,
        target_w = target_w,
        source_w = source_w,
    ), None);

    for unit in units {
        let state = if unit.kokomeco_present {
            "Resolved"
        } else {
            "Active"
        };
        let kokomeco = if unit.kokomeco_present {
            "Present"
        } else {
            "Missing"
        };
        color::print_info(&format!(
            "{:<integration_w$}  {:<target_w$}  {:<source_w$}  {:<8}  {:>7}  {:>8}  {:<8}",
            unit.integration,
            unit.target,
            unit.source,
            state,
            unit.pending,
            unit.resolved,
            kokomeco,
            integration_w = integration_w,
            target_w = target_w,
            source_w = source_w,
        ), None);
    }
}

fn print_orphaned_refs(orphaned_refs: &[String]) {
    color::print_warning("\nOrphaned MMM Refs (warning)", None);
    for reference in orphaned_refs {
        color::print_warning(&format!("  - {reference}"), None);
    }
}

fn select_current_branch_unit<'a>(units: &'a [StatusUnit], current_branch: &str) -> Option<&'a StatusUnit> {
    let mut matches = units
        .iter()
        .filter(|u| u.target == current_branch)
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return None;
    }

    matches.sort_by(|a, b| {
        let a_active = !a.kokomeco_present;
        let b_active = !b.kokomeco_present;
        b_active
            .cmp(&a_active)
            .then_with(|| a.integration.cmp(&b.integration))
    });
    matches.first().copied()
}

fn integration_from_kokomeco_branch(branch: &str) -> Option<String> {
    let parts = branch.split('/').collect::<Vec<_>>();
    if parts.len() == 4
        && parts[0] == "_mmm"
        && !parts[1].is_empty()
        && !parts[2].is_empty()
        && parts[3] == "kokomeco"
    {
        return Some(format!("_mmm/{}/{}/integration", parts[1], parts[2]));
    }
    None
}

fn slice_merge_status(
    integration_ref: &str,
    slice_branches: &[String],
) -> Result<BTreeMap<String, bool>> {
    let mut result = BTreeMap::new();
    for slice in slice_branches {
        let probe_ref = git_ops::best_ref_for_local_branch(slice)?.unwrap_or_else(|| slice.clone());
        result.insert(slice.clone(), git_ops::is_ancestor(&probe_ref, integration_ref)?);
    }
    Ok(result)
}

fn print_integration_status(
    integration_branch: &str,
    quiet: bool,
    current_branch: &str,
    tui_title: &str,
) -> Result<()> {
    let integration_ref =
        git_ops::best_ref_for_local_branch(integration_branch)?.unwrap_or_else(|| integration_branch.to_string());

    // If a kokomeco consolidated branch already exists for this integration
    // branch, show the merge suggestion instead of the raw integration status.
    let kokomeco = git_ops::consolidated_branch_name(&integration_branch);
    if git_ops::branch_exists_anywhere(&kokomeco)? {
        // Determine the intended target branch from the integration branch name.
        let expected_target =
            planner::parse_integration_branch(&integration_branch).map(|(target, _)| target);

        let target_mismatch = expected_target
            .as_ref()
            .is_some_and(|target| target != current_branch);

        if target_mismatch {
            let target = expected_target.as_ref().unwrap();
            if quiet {
                color::print_error(&format!(
                    "Warning: current branch '{}' does not match the integration target '{}'.",
                    current_branch, target
                ), None);
            } else {
                let prompt = format!(
                    "Current branch '{}' does not match the integration target '{}'.\n\n\
                     The kokomeco branch should be merged into '{}', not '{}'.\n\n\
                     Continue showing status?",
                    current_branch, target, target, current_branch
                );
                if !tui::confirm(&prompt, tui_title)? {
                    bail!("aborted: switch to '{}' before merging kokomeco", target);
                }
            }
        }

        let merge_target = expected_target.as_deref().unwrap_or(current_branch);
        let kokomeco_ref = git_ops::best_ref_for_local_branch(&kokomeco)?
            .unwrap_or_else(|| kokomeco.clone());

        println!("Mergetopus status");
        println!("  Integration branch:  {integration_branch}");
        println!("  Consolidated branch: {kokomeco}");
        println!();
        if target_mismatch {
            println!(
                "  ⚠ Current branch '{}' is NOT the integration target '{}'.",
                current_branch, merge_target
            );
            println!();
        }
        println!(
            "All slices are resolved. The kokomeco branch is ready to merge into '{merge_target}'."
        );
        println!();
        println!("Suggested next command:");
        if target_mismatch {
            println!("  git checkout {merge_target} && git merge {kokomeco_ref}");
        } else {
            println!("  git merge {kokomeco_ref}");
        }
        println!();
        println!("To clean up slice and integration branches afterward:");
        println!("  mergetopus cleanup");
        return Ok(());
    }

    let initial_commit = git_ops::first_mergetopus_partial_merge_commit(&integration_ref)?;
    let initial_message = git_ops::commit_message(&initial_commit)?;
    let parents = git_ops::commit_parent_shas(&initial_commit)?;

    let source_sha = parents
        .get(1)
        .cloned()
        .unwrap_or_else(|| "(unknown)".to_string());
    let source_ref =
        parse_partial_merge_source_ref(&initial_message).unwrap_or_else(|| "(unknown)".to_string());

    let slices = git_ops::list_slice_branches_for_integration(&integration_branch)?;
    let status = slice_merge_status(&integration_ref, &slices)?;

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

            let slice_ref =
                git_ops::best_ref_for_local_branch(slice)?.unwrap_or_else(|| slice.to_string());

            let tip_msg = git_ops::branch_tip_commit_message(&slice_ref)?;
            let mut paths = extract_slice_paths(&tip_msg);
            let resolve_tip = tip_msg
                .lines()
                .next()
                .unwrap_or("")
                .contains("Mergetopus resolve:");
            if paths.is_empty() && resolve_tip {
                let parent = git_ops::parent_sha(&slice_ref)?;
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

fn resolve_status_integration_branch(source: &str, current_branch: &str) -> Result<String> {
    let target = if planner::parse_integration_branch(source).is_some() {
        source.to_string()
    } else if let Some((_, current_source)) = planner::parse_integration_branch(current_branch) {
        if current_source == planner::sanitize_branch_fragment(source) {
            current_branch.to_string()
        } else {
            planner::integration_branch_name(current_branch, source)
        }
    } else {
        planner::integration_branch_name(current_branch, source)
    };

    if !git_ops::branch_exists_anywhere(&target)? {
        bail!(
            "could not find integration branch '{}'; provide an existing integration branch or source ref",
            target
        );
    }

    Ok(target)
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

    #[test]
    fn kokomeco_branch_maps_to_integration() {
        assert_eq!(
            integration_from_kokomeco_branch("_mmm/main/feature/kokomeco"),
            Some("_mmm/main/feature/integration".to_string())
        );
        assert_eq!(integration_from_kokomeco_branch("_mmm/main/feature/slice1"), None);
    }

    #[test]
    fn canonicalize_branch_ref_keeps_local_mmm_and_normalizes_remote() {
        let remotes = vec!["origin".to_string(), "upstream".to_string()];
        assert_eq!(
            canonicalize_branch_ref("_mmm/main/feature/integration", &remotes),
            "_mmm/main/feature/integration"
        );
        assert_eq!(
            canonicalize_branch_ref("origin/_mmm/main/feature/integration", &remotes),
            "_mmm/main/feature/integration"
        );
    }
}
