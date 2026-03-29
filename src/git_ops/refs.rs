use crate::git_ops::run_git;
use anyhow::{Context, Result};

pub fn head_sha() -> Result<String> {
    run_git(&["rev-parse", "--verify", "HEAD"])
}

pub fn resolve_commit(rev: &str) -> Result<String> {
    run_git(&["rev-parse", "--verify", &format!("{rev}^{{commit}}")])
        .with_context(|| format!("merge source '{rev}' is not a valid commit-ish ref"))
}

pub fn resolve_ref(reference: &str) -> Result<String> {
    run_git(&["rev-parse", "--verify", &format!("{reference}^{{commit}}")])
        .with_context(|| format!("failed to resolve reference '{reference}' to a commit"))
}
