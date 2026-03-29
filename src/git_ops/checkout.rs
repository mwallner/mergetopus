use crate::git_ops::{run_git, worktree};
use anyhow::Result;

pub fn checkout(branch: &str) -> Result<()> {
    let entries = worktree::list_worktree_entries()?;
    if worktree::has_existing_linked_worktrees(&entries) {
        let path = worktree::ensure_worktree_for_existing_branch(branch, &entries)?;
        worktree::switch_to_dir(&path)?;
    }

    run_git(&["checkout", branch]).map(|_| ())
}
