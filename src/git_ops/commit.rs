use crate::git_ops::run_git;
use anyhow::Result;

pub fn commit(message: &str) -> Result<()> {
    run_git(&["commit", "--allow-empty", "-m", message]).map(|_| ())
}

pub fn commit_strict(message: &str) -> Result<()> {
    run_git(&["commit", "-m", message]).map(|_| ())
}
