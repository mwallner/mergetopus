use crate::color;
use crate::forges::detect::{detect_forge, parse_remote_url};
use anyhow::{Result, bail};

use crate::git_ops;
use crate::planner;
use crate::tui;
use crate::tui_progress;

/// Push an initialized merge plan (integration + slices + kokomeco) to a remote.
///
/// Decision flow:
/// 1. Determine which integration branch to push (current branch or TUI picker).
/// 2. Determine which remote to push to (explicit arg, auto if 1 remote, TUI if >1).
/// 3. Verify source and target branches exist on the remote.
/// 4. Push integration + slices + kokomeco (if present) with --force-with-lease.
/// 5. Optionally create PRs for the pushed branches (--pr flag).
pub fn push_command(
    remote_arg: Option<&str>,
    create_prs: bool,
    quiet: bool,
    current_branch: &str,
    tui_title: &str,
) -> Result<()> {
    // --- Step 1: Determine integration branch ---
    let integration_branch =
        resolve_integration_branch(remote_arg, quiet, current_branch, tui_title)?;

    // --- Step 2: Determine remote ---
    let remote = resolve_remote(remote_arg, quiet, tui_title)?;

    // --- Step 3: Parse target and source from integration branch name ---
    let (safe_target, safe_source) = planner::parse_integration_branch(&integration_branch)
        .ok_or_else(|| {
            anyhow::anyhow!("'{}' is not a valid integration branch", integration_branch)
        })?;

    // Verify source and target exist on remote (check remote-tracking refs)
    let remote_target_ref = format!("{remote}/{safe_target}");
    if !git_ops::remote_branch_exists(&remote_target_ref)? {
        bail!(
            "target branch '{}' not found on remote '{}' — push it first",
            safe_target,
            remote
        );
    }

    let remote_source_ref = format!("{remote}/{safe_source}");
    if !git_ops::remote_branch_exists(&remote_source_ref)? {
        bail!(
            "source branch '{}' not found on remote '{}' — push it first",
            safe_source,
            remote
        );
    }

    // --- Step 4: Collect all branches to push ---
    let slices = git_ops::list_slice_branches_for_integration(&integration_branch)?;
    let kokomeco = git_ops::consolidated_branch_name(&integration_branch);
    let kokomeco_exists = git_ops::branch_exists(&kokomeco)?;

    let mut to_push: Vec<String> = Vec::new();
    to_push.push(integration_branch.clone());
    to_push.extend(slices.clone());
    if kokomeco_exists {
        to_push.push(kokomeco);
    }

    // --- Step 5: Push each branch ---
    if !quiet {
        let r = remote.clone();
        let steps: Vec<tui_progress::ProgressStep> = to_push
            .iter()
            .map(|branch| {
                let branch = branch.clone();
                let remote = r.clone();
                tui_progress::ProgressStep {
                    label: format!("Pushing {branch} \u{2192} {remote}"),
                    action: Box::new(move || git_ops::push_branch_force(&remote, &branch)),
                }
            })
            .collect();
        tui_progress::run_progress(tui_title, steps)?;
    } else {
        for branch in &to_push {
            color::print_info(&format!("Pushing {branch} \u{2192} {remote}"), None);
            git_ops::push_branch_force(&remote, branch)?;
        }
    }

    color::print_success(
        &format!("\nPushed {} branch(es) to '{remote}'.", to_push.len()),
        None,
    );

    // --- Step 6: Create PRs (if --pr flag set) ---
    if create_prs {
        create_prs_for_plan(
            &remote,
            &integration_branch,
            &slices,
            &safe_target,
            &safe_source,
            quiet,
        )?;
    }

    Ok(())
}

fn create_prs_for_plan(
    remote: &str,
    integration_branch: &str,
    slices: &[String],
    target: &str,
    source: &str,
    quiet: bool,
) -> Result<()> {
    let remote_url = git_ops::get_remote_url(remote)?;
    let forge = detect_forge(&remote_url)?;
    let info = parse_remote_url(&remote_url)?;
    let repo_path = format!("{}/{}", info.owner, info.repo);

    let all_branches: Vec<&str> = std::iter::once(integration_branch)
        .chain(slices.iter().map(|s| s.as_str()))
        .collect();

    for branch in &all_branches {
        let base = if *branch == integration_branch {
            target
        } else {
            integration_branch
        };

        let title = if *branch == integration_branch {
            format!("[MMM] Integration: {source} \u{2192} {target}")
        } else {
            format!("[MMM] Slice: {branch}")
        };

        let body = if *branch == integration_branch {
            format!("Mergetopus integration branch merging **{source}** into **{target}**.\n\nAll slice branches must be resolved before this PR can be merged.")
        } else {
            format!("Mergetopus slice branch for merging **{source}** into **{target}**.\n\nBranch: `{branch}`")
        };

        let existing = forge.find_pr_by_head(&repo_path, branch)?;

        if let Some(pr) = existing {
            if !quiet {
                color::print_info(
                    &format!("PR already exists for {branch}: #{} ({})", pr.number, pr.html_url),
                    None,
                );
            }
        } else {
            forge.create_pr(crate::forges::PrParams {
                owner: info.owner.clone(),
                repo: info.repo.clone(),
                title,
                body,
                head: branch.to_string(),
                base: base.to_string(),
                draft: true,
                labels: vec![],
            })?;
            if !quiet {
                color::print_success(&format!("Created PR for {branch}"), None);
            }
        }
    }

    Ok(())
}

/// Returns the integration branch to operate on.
///
/// - If `current_branch` is an integration branch, use it.
/// - Otherwise, list local unpushed integration branches for the resolved remote
///   and offer a TUI picker (error if none exist; error in --quiet mode).
fn resolve_integration_branch(
    remote_arg: Option<&str>,
    quiet: bool,
    current_branch: &str,
    tui_title: &str,
) -> Result<String> {
    if planner::parse_integration_branch(current_branch).is_some() {
        return Ok(current_branch.to_string());
    }

    // We need a remote to filter "not yet pushed" branches.  Resolve it first
    // (without a TUI so we can fall back gracefully when quiet).
    let remote_for_filter = match remote_arg {
        Some(r) => r.to_string(),
        None => {
            let remotes = git_ops::list_remote_names()?;
            match remotes.len() {
                0 => bail!("no remotes configured"),
                1 => remotes.into_iter().next().unwrap(),
                _ => {
                    if quiet {
                        bail!(
                            "multiple remotes configured — pass REMOTE explicitly in --quiet mode"
                        );
                    }
                    // We'll pick the remote properly later; for the unpushed filter
                    // we use the first one as a best-effort heuristic, since the
                    // remote TUI comes after this step.  The filter only needs to
                    // exclude branches already on *some* remote.
                    remotes.into_iter().next().unwrap()
                }
            }
        }
    };

    let candidates = git_ops::list_unpushed_integration_branches(&remote_for_filter)?;

    if candidates.is_empty() {
        bail!(
            "no unpushed local integration branches found (current branch '{}' is not an integration branch)",
            current_branch
        );
    }

    if quiet {
        bail!(
            "current branch '{}' is not an integration branch; pass an explicit integration branch or re-run without --quiet",
            current_branch
        );
    }

    let picked = tui::pick_branch(&candidates, tui_title, None, &[])?;
    picked.ok_or_else(|| anyhow::anyhow!("no integration branch selected"))
}

/// Returns the remote name to use.
///
/// - Explicit `remote_arg` wins.
/// - Single remote: use automatically.
/// - Multiple remotes: TUI picker (error in --quiet mode).
fn resolve_remote(remote_arg: Option<&str>, quiet: bool, tui_title: &str) -> Result<String> {
    if let Some(r) = remote_arg {
        return Ok(r.to_string());
    }

    let remotes = git_ops::list_remote_names()?;
    match remotes.len() {
        0 => bail!("no remotes configured"),
        1 => Ok(remotes.into_iter().next().unwrap()),
        _ => {
            if quiet {
                bail!("multiple remotes configured — pass REMOTE explicitly in --quiet mode");
            }
            let picked = tui::pick_branch(&remotes, tui_title, None, &[])?;
            picked.ok_or_else(|| anyhow::anyhow!("no remote selected"))
        }
    }
}
