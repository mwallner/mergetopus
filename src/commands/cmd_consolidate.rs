use crate::color;
use crate::git_ops;
use crate::planner;
use anyhow::{Context, Result, bail};

use super::cmd_merge_workflow;

/// Create a consolidated kokomeco merge commit branch.
///
/// When on an integration branch, the source ref is parsed from the branch name.
/// When on any other branch, SOURCE must be provided explicitly.
pub fn consolidate_command(source: Option<&str>, quiet: bool, current_branch: &str) -> Result<()> {
    let (integration_branch, source_ref) =
        resolve_integration_and_source(current_branch, source)?;

    if !git_ops::branch_exists_anywhere(&integration_branch)? {
        bail!(
            "no merge context found for '{}' (integration branch '{integration_branch}' does not exist)",
            if let Some(s) = source { s } else { current_branch }
        );
    }

    let local_integration = git_ops::ensure_local_branch_for_operation(&integration_branch)?;
    let slices = git_ops::list_slice_branches_for_integration(&local_integration)?;
    let status = git_ops::slice_merge_status(&local_integration, &slices)?;

    let all_merged = status.values().all(|v| *v);
    if !all_merged {
        color::print_emphasis("Pending slice merges:", None);
        for (slice, merged) in &status {
            if *merged {
                color::print_info(&format!("  - {slice}: merged"), None);
            } else {
                color::print_warning(&format!("  - {slice}: pending"), None);
            }
        }
        if slices.is_empty() {
            color::print_info("  (no slice branches found)", None);
        }
        bail!("cannot consolidate: all slice branches must be merged into integration first");
    }

    if !quiet {
        color::print_info(&format!(
            "All slices are merged into '{local_integration}'. Creating kokomeco branch ...",
        ), None);
    }

    let branch = cmd_merge_workflow::create_consolidated_merge_commit_branch(
        &local_integration,
        &source_ref,
        &status,
    )
    .with_context(|| "failed to create consolidated merge commit branch")?;

    color::print_success(&format!("Created kokomeco branch: {branch}"), None);
    color::print_info(
        "Integration branch was not rewritten. Review and promote explicitly if desired.",
        None,
    );

    Ok(())
}

/// Resolve the integration branch name and source ref from the current context.
///
/// Returns `(integration_branch, source_ref)`.
fn resolve_integration_and_source(
    current_branch: &str,
    source: Option<&str>,
) -> Result<(String, String)> {
    if let Some((_target, source)) = planner::parse_integration_branch(current_branch) {
        // Already on an integration branch — use it directly.
        return Ok((current_branch.to_string(), source));
    }

    let src = match source {
        Some(s) => s.to_string(),
        None => bail!(
            "cannot determine merge context: use 'mergetopus consolidate <SOURCE>' from a target branch, \
             or switch to an integration branch"
        ),
    };

    let integration_branch = planner::integration_branch_name(current_branch, &src);
    Ok((integration_branch, src))
}
