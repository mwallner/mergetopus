pub mod auth;
pub mod bitbucket;
pub mod detect;
pub mod forgejo;
pub mod github;
pub mod gitlab;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeId {
    GitHub,
    GitLab,
    Bitbucket,
    Forgejo,
}

impl ForgeId {
    pub fn name(&self) -> &str {
        match self {
            ForgeId::GitHub => "GitHub",
            ForgeId::GitLab => "GitLab",
            ForgeId::Bitbucket => "Bitbucket",
            ForgeId::Forgejo => "Forgejo",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrParams {
    pub owner: String,
    pub repo: String,
    pub title: String,
    pub body: String,
    pub head: String,
    pub base: String,
    pub draft: bool,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrUpdate {
    pub title: Option<String>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

impl std::fmt::Display for PrState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrState::Open => write!(f, "open"),
            PrState::Closed => write!(f, "closed"),
            PrState::Merged => write!(f, "merged"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub html_url: String,
    pub state: PrState,
    pub draft: bool,
    pub head: String,
    pub base: String,
}

pub trait Forge: Send + Sync {
    fn id(&self) -> ForgeId;
    fn name(&self) -> &str;
    fn base_url(&self) -> &str;

    fn create_pr(&self, params: PrParams) -> Result<PullRequest>;
    fn update_pr(&self, repo_path: &str, number: u64, params: PrUpdate) -> Result<PullRequest>;
    fn find_pr_by_head(&self, repo_path: &str, head: &str) -> Result<Option<PullRequest>>;
    fn close_pr(&self, repo_path: &str, number: u64) -> Result<PullRequest>;
}

pub(crate) fn url_encode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push_str("%20"),
            _ => result.push_str(&format!("%{byte:02X}")),
        }
    }
    result
}
