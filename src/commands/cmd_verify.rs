use anyhow::{Result, bail};
use crate::color;

use crate::git_ops;
use crate::planner;

pub fn verify_command(source_arg: Option<&str>, global: bool, current_branch: &str) -> Result<()> {
    if global {
        return verify_global_command();
    }

    let integration_branch = resolve_verify_integration_branch(source_arg, current_branch)?;
    let newer = find_newer_integration_commits(&integration_branch)?;

    if !newer.is_empty() {
        let kokomeco_branch = git_ops::consolidated_branch_name(&integration_branch);
        let preview = newer
            .iter()
            .take(5)
            .map(|sha| format!("  - {sha}"))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "verification failed: integration branch '{}' has {} commit(s) newer than kokomeco '{}'.\n{}\nrecreate kokomeco after finishing integration changes",
            integration_branch,
            newer.len(),
            kokomeco_branch,
            preview
        );
    }

    let kokomeco_branch = git_ops::consolidated_branch_name(&integration_branch);

    color::print_emphasis("Mergetopus verify", None);
    color::print_info(&format!("  Integration branch: {integration_branch}"), None);
    color::print_info(&format!("  Kokomeco branch:    {kokomeco_branch}"), None);
    color::print_success("verification passed: no integration commits newer than kokomeco", None);

    Ok(())
}

fn verify_global_command() -> Result<()> {
    let integrations = discover_integration_branches()?;
    if integrations.is_empty() {
        color::print_emphasis("Mergetopus verify --global", None);
        color::print_info("No integration branches found under '_mmm/' across local and configured remotes.", None);
        return Ok(());
    }

    let mut checked = 0usize;
    let mut skipped_missing_kokomeco = 0usize;
    let mut failures = Vec::new();

    for integration_branch in integrations {
        let kokomeco_branch = git_ops::consolidated_branch_name(&integration_branch);
        if !git_ops::branch_exists_anywhere(&kokomeco_branch)? {
            skipped_missing_kokomeco += 1;
            continue;
        }

        checked += 1;
        let newer = find_newer_integration_commits(&integration_branch)?;
        if !newer.is_empty() {
            failures.push((integration_branch, kokomeco_branch, newer));
        }
    }

    if !failures.is_empty() {
        let details = failures
            .iter()
            .map(|(integration, kokomeco, newer)| {
                let preview = newer
                    .iter()
                    .take(5)
                    .map(|sha| format!("    - {sha}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "  {} -> {} has {} newer commit(s):\n{}",
                    integration,
                    kokomeco,
                    newer.len(),
                    preview
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        bail!(
            "global verification failed for {} integration branch(es) (checked {}, skipped {} without kokomeco).\n{}\nrecreate kokomeco after finishing integration changes",
            failures.len(),
            checked,
            skipped_missing_kokomeco,
            details
        );
    }

    color::print_emphasis("Mergetopus verify --global", None);
    color::print_success(&format!(
        "verification passed: checked {} integration branch(es); skipped {} without kokomeco",
        checked,
        skipped_missing_kokomeco
    ), None);

    Ok(())
}

fn discover_integration_branches() -> Result<Vec<String>> {
    let refs = git_ops::list_branch_refs()?;
    let remote_names = git_ops::list_remote_names()?;
    let mut integrations = std::collections::BTreeSet::new();

    for reference in refs {
        let canonical = canonicalize_branch_ref(&reference, &remote_names);
        if planner::parse_integration_branch(&canonical).is_some() {
            integrations.insert(canonical);
        }
    }

    Ok(integrations.into_iter().collect())
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

fn find_newer_integration_commits(integration_branch: &str) -> Result<Vec<String>> {
    let integration_ref = git_ops::best_ref_for_local_branch(&integration_branch)?
        .unwrap_or_else(|| integration_branch.to_string());

    let kokomeco_branch = git_ops::consolidated_branch_name(&integration_branch);
    if !git_ops::branch_exists_anywhere(&kokomeco_branch)? {
        bail!(
            "could not verify '{}': missing kokomeco branch '{}'",
            integration_branch,
            kokomeco_branch
        );
    }

    let kokomeco_ref = git_ops::best_ref_for_local_branch(&kokomeco_branch)?
        .unwrap_or_else(|| kokomeco_branch.clone());

    let kokomeco_ts_raw = git_ops::run_git(&["show", "-s", "--format=%ct", &kokomeco_ref])?;
    let kokomeco_ts: i64 = kokomeco_ts_raw
        .trim()
        .parse()
        .map_err(|e| anyhow::anyhow!("failed to parse kokomeco timestamp '{}': {e}", kokomeco_ts_raw.trim()))?;

    let integration_log = git_ops::run_git(&[
        "log",
        "--first-parent",
        "--format=%H%x1f%ct",
        &integration_ref,
    ])?;

    let mut newer = Vec::new();
    for line in integration_log.lines() {
        let mut parts = line.split('\u{1f}');
        let sha = parts.next().unwrap_or("").trim();
        let ts_raw = parts.next().unwrap_or("").trim();
        if sha.is_empty() || ts_raw.is_empty() {
            continue;
        }

        let ts: i64 = match ts_raw.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };

        if ts > kokomeco_ts {
            newer.push(sha.to_string());
        }
    }

    Ok(newer)
}

fn resolve_verify_integration_branch(source_arg: Option<&str>, current_branch: &str) -> Result<String> {
    if let Some(source) = source_arg {
        let target = if planner::parse_integration_branch(source).is_some() {
            source.to_string()
        } else if let Some((_, current_source)) = planner::parse_integration_branch(current_branch)
        {
            if current_source == planner::sanitize_branch_fragment_legacy(source) {
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

        return Ok(target);
    }

    if planner::parse_integration_branch(current_branch).is_some() {
        return Ok(current_branch.to_string());
    }

    bail!(
        "verify requires SOURCE/integration branch unless the current branch is an integration branch. Tip: use --global to validate all integration branches with an existing kokomeco branch."
    )
}
