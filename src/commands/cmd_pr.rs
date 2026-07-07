use anyhow::{Result, bail};

use crate::color;
use crate::forges;
use crate::forges::detect::{detect_forge, parse_remote_url};
use crate::git_ops;
use crate::planner;
use crate::tui;

pub fn pr_command(
    action: PrAction,
    source: Option<&str>,
    quiet: bool,
    current_branch: &str,
    tui_title: &str,
) -> Result<()> {
    let integration_branch =
        resolve_integration_branch(source, quiet, current_branch, tui_title)?;

    let (safe_target, safe_source) = planner::parse_integration_branch(&integration_branch)
        .ok_or_else(|| anyhow::anyhow!("'{}' is not a valid integration branch", integration_branch))?;

    let remote = resolve_remote(None, quiet, tui_title)?;
    let remote_url = git_ops::get_remote_url(&remote)?;
    let forge = detect_forge(&remote_url)?;
    let info = parse_remote_url(&remote_url)?;

    exec_pr_action(
        forge.as_ref(),
        &info.owner,
        &info.repo,
        &integration_branch,
        &safe_target,
        &safe_source,
        &remote,
        action,
    )
}

/// Core PR logic — accepts an already-resolved forge for testability.
pub(crate) fn exec_pr_action(
    forge: &dyn forges::Forge,
    owner: &str,
    repo: &str,
    integration_branch: &str,
    target: &str,
    source: &str,
    remote_name: &str,
    action: PrAction,
) -> Result<()> {
    let slices = git_ops::list_slice_branches_for_integration(integration_branch)?;
    let kokomeco = git_ops::consolidated_branch_name(integration_branch);
    let kokomeco_exists = git_ops::branch_exists_anywhere(&kokomeco)?;
    let kokomeco_branch = if kokomeco_exists { Some(kokomeco.as_str()) } else { None };
    exec_pr_action_inner(forge, owner, repo, integration_branch, &slices, kokomeco_branch, target, source, remote_name, action)
}

/// Like `exec_pr_action` but accepts pre-resolved slice list and optional kokomeco branch
/// (testable without git).
fn exec_pr_action_inner(
    forge: &dyn forges::Forge,
    owner: &str,
    repo: &str,
    integration_branch: &str,
    slices: &[String],
    kokomeco_branch: Option<&str>,
    target: &str,
    source: &str,
    remote_name: &str,
    action: PrAction,
) -> Result<()> {

    let integration_pr = find_or_create_integration_pr(
        forge,
        owner,
        repo,
        integration_branch,
        target,
        source,
        action,
    )?;

    let mut results: Vec<PrResult> = Vec::new();

    if let Some(pr) = integration_pr {
        results.push(PrResult {
            branch: integration_branch.to_string(),
            action: action.label().to_string(),
            pr,
        });
    }

    for slice in slices {
        let pr = find_or_create_slice_pr(
            forge,
            owner,
            repo,
            slice,
            integration_branch,
            target,
            source,
            action,
        )?;
        if let Some(pr) = pr {
            results.push(PrResult {
                branch: slice.clone(),
                action: action.label().to_string(),
                pr,
            });
        }
    }

    // If a kokomeco branch exists for this integration, include it as a PR.
    if let Some(kokomeco) = kokomeco_branch {
        if let Some(pr) = find_or_create_kokomeco_pr(
            forge,
            owner,
            repo,
            kokomeco,
            target,
            source,
            action,
        )? {
            results.push(PrResult {
                branch: kokomeco.to_string(),
                action: action.label().to_string(),
                pr,
            });
        }
    }

    for r in &results {
        let status = match r.pr.state {
            forges::PrState::Open => "open",
            forges::PrState::Closed => "closed",
            forges::PrState::Merged => "merged",
        };
        color::print_info(
            &format!("{} {} #{} ({status}): {}", r.action, r.branch, r.pr.number, r.pr.html_url),
            None,
        );
    }

    if results.is_empty() {
        color::print_info("No pull requests to manage.", None);
    } else {
        color::print_success(
            &format!("\n{} pull request(s) for '{source}' on '{remote_name}'.", results.len()),
            None,
        );
    }

    Ok(())
}

fn find_or_create_pr(
    forge: &dyn forges::Forge,
    owner: &str,
    repo: &str,
    head: &str,
    base: &str,
    title: &str,
    body: &str,
    action: PrAction,
) -> Result<Option<forges::PullRequest>> {
    let repo_path = format!("{owner}/{repo}");
    match action {
        PrAction::Create | PrAction::Sync => {
            if let Some(existing) = forge.find_pr_by_head(&repo_path, head)? {
                if matches!(action, PrAction::Sync) {
                    let updated = forge.update_pr(&repo_path, existing.number, forges::PrUpdate {
                        title: Some(title.to_string()),
                        body: Some(body.to_string()),
                    })?;
                    return Ok(Some(updated));
                }
                return Ok(Some(existing));
            }

            let pr = forge.create_pr(forges::PrParams {
                owner: owner.to_string(),
                repo: repo.to_string(),
                title: title.to_string(),
                body: body.to_string(),
                head: head.to_string(),
                base: base.to_string(),
                draft: true,
                labels: vec![],
            })?;
            Ok(Some(pr))
        }
        PrAction::List => {
            if let Some(existing) = forge.find_pr_by_head(&repo_path, head)? {
                Ok(Some(existing))
            } else {
                Ok(None)
            }
        }
    }
}

fn find_or_create_integration_pr(
    forge: &dyn forges::Forge,
    owner: &str,
    repo: &str,
    integration_branch: &str,
    target: &str,
    source: &str,
    action: PrAction,
) -> Result<Option<forges::PullRequest>> {
    find_or_create_pr(
        forge,
        owner,
        repo,
        integration_branch,
        target,
        &format!("[MMM] Integration: {source} \u{2192} {target}"),
        &format!("Mergetopus integration branch merging **{source}** into **{target}**.\n\nAll slice branches must be resolved before this PR can be merged."),
        action,
    )
}

fn find_or_create_slice_pr(
    forge: &dyn forges::Forge,
    owner: &str,
    repo: &str,
    slice_branch: &str,
    integration_branch: &str,
    _target: &str,
    _source: &str,
    action: PrAction,
) -> Result<Option<forges::PullRequest>> {
    find_or_create_pr(
        forge,
        owner,
        repo,
        slice_branch,
        integration_branch,
        &format!("[MMM] Slice: {slice_branch}"),
        &format!("Mergetopus slice branch for merging **{_source}** into **{_target}**.\n\nBranch: `{slice_branch}`"),
        action,
    )
}

fn find_or_create_kokomeco_pr(
    forge: &dyn forges::Forge,
    owner: &str,
    repo: &str,
    kokomeco_branch: &str,
    target: &str,
    source: &str,
    action: PrAction,
) -> Result<Option<forges::PullRequest>> {
    find_or_create_pr(
        forge,
        owner,
        repo,
        kokomeco_branch,
        target,
        &format!("[MMM] Consolidated: {source} \u{2192} {target}"),
        &format!(
            "Mergetopus consolidated merge branch for **{source}** into **{target}**.\n\n\
             This branch contains the resolved integration tree as a proper merge commit.\n\n\
             Branch: `{kokomeco_branch}`"
        ),
        action,
    )
}

fn resolve_integration_branch(
    source: Option<&str>,
    quiet: bool,
    current_branch: &str,
    tui_title: &str,
) -> Result<String> {
    if let Some(src) = source {
        let branch = if planner::parse_integration_branch(src).is_some() {
            src.to_string()
        } else {
            planner::integration_branch_name(current_branch, src)
        };
        if !git_ops::branch_exists_anywhere(&branch)? {
            bail!("integration branch '{branch}' not found");
        }
        return Ok(branch);
    }

    if planner::parse_integration_branch(current_branch).is_some() {
        return Ok(current_branch.to_string());
    }

    let all_local = git_ops::list_local_branches()?;
    let candidates: Vec<String> = all_local
        .into_iter()
        .filter(|b| planner::parse_integration_branch(b).is_some())
        .collect();

    if candidates.is_empty() {
        bail!(
            "no integration branch found (current branch '{}' is not an integration branch)",
            current_branch
        );
    }

    if quiet {
        bail!(
            "current branch '{}' is not an integration branch; pass SOURCE or re-run without --quiet",
            current_branch
        );
    }

    let picked = tui::pick_branch(&candidates, tui_title, None, &[])?;
    picked.ok_or_else(|| anyhow::anyhow!("no integration branch selected"))
}

fn resolve_remote(remote_arg: Option<&str>, quiet: bool, tui_title: &str) -> Result<String> {
    if let Some(r) = remote_arg {
        return Ok(r.to_string());
    }

    let remotes = git_ops::list_remote_names()?;
    match remotes.len() {
        0 => bail!("no remotes configured"),
        1 => Ok(remotes.into_iter().next().expect("just checked len == 1")),
        _ => {
            if quiet {
                bail!("multiple remotes configured — pass REMOTE explicitly in --quiet mode");
            }
            let picked = tui::pick_branch(&remotes, tui_title, None, &[])?;
            picked.ok_or_else(|| anyhow::anyhow!("no remote selected"))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrAction {
    Create,
    Sync,
    List,
}

impl PrAction {
    fn label(&self) -> &str {
        match self {
            PrAction::Create => "created",
            PrAction::Sync => "synced",
            PrAction::List => "found",
        }
    }
}

struct PrResult {
    branch: String,
    action: String,
    pr: forges::PullRequest,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forges::{self, Forge, ForgeId, PrParams, PrUpdate, PrState};
    use std::sync::Mutex;

    type PullRequest = forges::PullRequest;

    struct MockForge {
        existing_prs: Mutex<Vec<PullRequest>>,
        next_number: Mutex<u64>,
    }

    impl MockForge {
        fn new(existing: Vec<PullRequest>) -> Self {
            MockForge {
                existing_prs: Mutex::new(existing),
                next_number: Mutex::new(100),
            }
        }
    }

    impl Forge for MockForge {
        fn id(&self) -> ForgeId { ForgeId::GitHub }
        fn name(&self) -> &str { "MockForge" }
        fn base_url(&self) -> &str { "https://mock.example.com" }

        fn create_pr(&self, params: PrParams) -> Result<PullRequest> {
            let mut prs = self.existing_prs.lock().unwrap();
            let mut num = self.next_number.lock().unwrap();
            let number = *num;
            *num += 1;
            let pr = PullRequest {
                number,
                html_url: format!("https://mock.example.com/pr/{number}"),
                state: PrState::Open,
                draft: params.draft,
                head: params.head,
                base: params.base,
            };
            prs.push(pr.clone());
            Ok(pr)
        }

        fn update_pr(&self, _repo_path: &str, number: u64, _params: PrUpdate) -> Result<PullRequest> {
            Ok(PullRequest {
                number,
                html_url: format!("https://mock.example.com/pr/{number}"),
                state: PrState::Open,
                draft: false,
                head: "updated-head".into(),
                base: "updated-base".into(),
            })
        }

        fn find_pr_by_head(&self, _repo_path: &str, head: &str) -> Result<Option<PullRequest>> {
            let prs = self.existing_prs.lock().unwrap();
            Ok(prs.iter().find(|pr| pr.head == head).cloned())
        }

        fn close_pr(&self, _repo_path: &str, number: u64) -> Result<PullRequest> {
            let mut prs = self.existing_prs.lock().unwrap();
            if let Some(pr) = prs.iter_mut().find(|pr| pr.number == number) {
                pr.state = PrState::Closed;
            }
            Ok(PullRequest {
                number,
                html_url: format!("https://mock.example.com/pr/{number}"),
                state: PrState::Closed,
                draft: false,
                head: String::new(),
                base: String::new(),
            })
        }
    }

    #[test]
    fn mock_forge_create_and_find() {
        let forge = MockForge::new(vec![]);
        let pr = forge.create_pr(PrParams {
            owner: "o".into(), repo: "r".into(), title: "test".into(),
            body: "".into(), head: "feature".into(), base: "main".into(),
            draft: true, labels: vec![],
        }).unwrap();
        assert_eq!(pr.number, 100);
        assert_eq!(pr.head, "feature");
        assert_eq!(pr.base, "main");
        assert!(pr.draft);

        let found = forge.find_pr_by_head("o/r", "feature").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().number, 100);

        assert!(forge.find_pr_by_head("o/r", "other").unwrap().is_none());
    }

    #[test]
    fn mock_forge_close() {
        let forge = MockForge::new(vec![]);
        let pr = forge.create_pr(PrParams {
            owner: "o".into(), repo: "r".into(), title: "x".into(),
            body: "".into(), head: "branch".into(), base: "main".into(),
            draft: false, labels: vec![],
        }).unwrap();
        let closed = forge.close_pr("o/r", pr.number).unwrap();
        assert_eq!(closed.state, PrState::Closed);
        let found = forge.find_pr_by_head("o/r", "branch").unwrap().unwrap();
        assert_eq!(found.state, PrState::Closed);
    }

    #[test]
    fn exec_pr_action_inner_list_no_prs() {
        let forge = MockForge::new(vec![]);
        exec_pr_action_inner(
            &forge, "o", "r", "_mmm/main/ts/integration", &[], None, "main", "ts", "origin",
            PrAction::List,
        ).unwrap();
    }

    #[test]
    fn exec_pr_action_inner_creates_draft_prs() {
        let forge = MockForge::new(vec![]);
        exec_pr_action_inner(
            &forge, "o", "r", "_mmm/main/ts/integration", &["_mmm/main/ts/slice1".to_string()], None, "main", "ts", "origin",
            PrAction::Create,
        ).unwrap();

        let int_pr = forge.find_pr_by_head("o/r", "_mmm/main/ts/integration").unwrap();
        assert!(int_pr.is_some());
        assert!(int_pr.unwrap().draft);

        let slice_pr = forge.find_pr_by_head("o/r", "_mmm/main/ts/slice1").unwrap();
        assert!(slice_pr.is_some());
        assert!(slice_pr.unwrap().draft);
    }

    #[test]
    fn exec_pr_action_inner_list_finds_existing() {
        let existing = vec![PullRequest {
            number: 1,
            html_url: "https://mock.example.com/pr/1".into(),
            state: PrState::Open,
            draft: true,
            head: "_mmm/main/ts/integration".into(),
            base: "main".into(),
        }];
        let forge = MockForge::new(existing);
        exec_pr_action_inner(
            &forge, "o", "r", "_mmm/main/ts/integration", &[], None, "main", "ts", "origin",
            PrAction::List,
        ).unwrap();
    }

    #[test]
    fn exec_pr_action_inner_sync_updates_existing() {
        let existing = vec![PullRequest {
            number: 1,
            html_url: "https://mock.example.com/pr/1".into(),
            state: PrState::Open,
            draft: true,
            head: "_mmm/main/ts/integration".into(),
            base: "main".into(),
        }];
        let forge = MockForge::new(existing);
        exec_pr_action_inner(
            &forge, "o", "r", "_mmm/main/ts/integration", &[], None, "main", "ts", "origin",
            PrAction::Sync,
        ).unwrap();
    }

    #[test]
    fn exec_pr_action_inner_skips_create_when_exists() {
        let existing = vec![PullRequest {
            number: 1,
            html_url: "https://mock.example.com/pr/1".into(),
            state: PrState::Open,
            draft: true,
            head: "_mmm/main/ts/integration".into(),
            base: "main".into(),
        }];
        let forge = MockForge::new(existing);
        // Should not panic or create a duplicate — reuses existing PR.
        exec_pr_action_inner(
            &forge, "o", "r", "_mmm/main/ts/integration", &[], None, "main", "ts", "origin",
            PrAction::Create,
        ).unwrap();
        // Only the original PR exists.
        let all = forge.existing_prs.lock().unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn exec_pr_action_inner_creates_kokomeco_pr() {
        let forge = MockForge::new(vec![]);
        exec_pr_action_inner(
            &forge, "o", "r", "_mmm/main/ts/integration", &[], Some("_mmm/main/ts/kokomeco"), "main", "ts", "origin",
            PrAction::Create,
        ).unwrap();

        let kok_pr = forge.find_pr_by_head("o/r", "_mmm/main/ts/kokomeco").unwrap();
        assert!(kok_pr.is_some(), "kokomeco PR should be created");
        assert!(kok_pr.unwrap().draft);

        // Integration and slice PRs are still created too.
        let int_pr = forge.find_pr_by_head("o/r", "_mmm/main/ts/integration").unwrap();
        assert!(int_pr.is_some());
    }
}
