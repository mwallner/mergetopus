use crate::git_ops::{run_git, run_git_allow_failure};
use anyhow::{Context, Result, bail};

pub fn merge_in_progress() -> Result<bool> {
    let (ok, _, _) = run_git_allow_failure(&["rev-parse", "-q", "--verify", "MERGE_HEAD"])?;
    Ok(ok)
}

pub fn merge_head_sha() -> Result<String> {
    run_git(&["rev-parse", "--verify", "MERGE_HEAD"])
        .context("failed to resolve MERGE_HEAD for in-progress merge")
}

pub fn merge_no_commit(source: &str) -> Result<()> {
    let (ok, _, stderr) = run_git_allow_failure(&["merge", "--no-ff", "--no-commit", source])?;
    if ok {
        return Ok(());
    }

    // Expected conflict path: merge exits non-zero but leaves MERGE_HEAD.
    if merge_in_progress()? {
        return Ok(());
    }

    bail!(
        "git merge failed before entering conflict resolution: {}\n\
         verify source/history compatibility, then retry (for unrelated histories, merge manually with --allow-unrelated-histories first)",
        stderr
    );
}

pub fn merge_abort() -> Result<()> {
    run_git(&["merge", "--abort"]).map(|_| ())
}
