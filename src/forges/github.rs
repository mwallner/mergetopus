use anyhow::{Result, bail, Context};
use serde::Deserialize;

use super::{Forge, ForgeId, PrParams, PrUpdate, PullRequest, PrState};

type GhResponse = ureq::http::Response<ureq::Body>;

pub struct GitHub {
    token: String,
    owner: String,
    repo: String,
}

impl GitHub {
    pub fn new(token: String, owner: String, repo: String) -> Self {
        GitHub { token, owner, repo }
    }

    fn api_url(&self, path: &str) -> String {
        format!("https://api.github.com{path}")
    }

    fn repo_path(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    fn agent(&self) -> ureq::Agent {
        ureq::agent()
    }

    fn headers<B>(&self, req: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        req.header("Authorization", &format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }

    fn api_post<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        body: serde_json::Value,
    ) -> Result<T> {
        let resp = self
            .headers(self.agent().post(url))
            .send_json(body)
            .with_context(|| format!("POST {url} failed"))?;
        Self::handle_response(resp)
    }

    fn api_patch<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        body: serde_json::Value,
    ) -> Result<T> {
        let resp = self
            .headers(self.agent().patch(url))
            .send_json(body)
            .with_context(|| format!("PATCH {url} failed"))?;
        Self::handle_response(resp)
    }

    fn api_get<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<T> {
        let resp = self
            .headers(self.agent().get(url))
            .call()
            .with_context(|| format!("GET {url} failed"))?;
        Self::handle_response(resp)
    }

    fn handle_response<T: serde::de::DeserializeOwned>(mut resp: GhResponse) -> Result<T> {
        let status = resp.status().as_u16();
        match status {
            200 | 201 => {
                let body = resp.body_mut();
                Ok(body.read_json::<T>()?)
            }
            204 => Ok(serde_json::from_value(serde_json::json!({}))?),
            304 => bail!("resource not modified (cached)"),
            401 => bail!("GitHub API: authentication failed; check your token"),
            403 => bail!("GitHub API: forbidden or rate limited"),
            404 => bail!("GitHub API: resource not found (check owner/repo)"),
            422 => {
                let body_str = resp.body_mut().read_to_string().unwrap_or_default();
                bail!("GitHub API: validation error (HTTP 422): {body_str}")
            }
            s => {
                let body_str = resp.body_mut().read_to_string().unwrap_or_default();
                bail!("GitHub API: HTTP {s}: {body_str}")
            }
        }
    }
}

impl Forge for GitHub {
    fn id(&self) -> ForgeId {
        ForgeId::GitHub
    }

    fn name(&self) -> &str {
        "GitHub"
    }

    fn base_url(&self) -> &str {
        "https://api.github.com"
    }

    fn create_pr(&self, params: PrParams) -> Result<PullRequest> {
        let url = self.api_url(&format!(
            "/repos/{}/{}/pulls",
            self.owner, self.repo
        ));

        let mut body = serde_json::json!({
            "title": params.title,
            "head": params.head,
            "base": params.base,
            "draft": params.draft,
        });

        if !params.body.is_empty() {
            body["body"] = serde_json::json!(params.body);
        }

        let resp: GithubPrResponse = self.api_post(&url, body)?;
        Ok(convert_pr(resp))
    }

    fn update_pr(&self, repo_path: &str, number: u64, params: PrUpdate) -> Result<PullRequest> {
        let url = self.api_url(&format!("/repos/{repo_path}/pulls/{number}"));

        let mut body = serde_json::json!({});
        if let Some(title) = params.title {
            body["title"] = serde_json::json!(title);
        }
        if let Some(body_text) = params.body {
            body["body"] = serde_json::json!(body_text);
        }

        let resp: GithubPrResponse = self.api_patch(&url, body)?;
        Ok(convert_pr(resp))
    }

    fn find_pr_by_head(&self, repo_path: &str, head: &str) -> Result<Option<PullRequest>> {
        let url = self.api_url(&format!(
            "/repos/{repo_path}/pulls?head={}:{}&state=open",
            self.owner, head
        ));

        let resp: Vec<GithubPrResponse> = self.api_get(&url)?;
        Ok(resp.into_iter().next().map(convert_pr))
    }

    fn close_pr(&self, repo_path: &str, number: u64) -> Result<PullRequest> {
        let url = self.api_url(&format!("/repos/{repo_path}/pulls/{number}"));
        let body = serde_json::json!({"state": "closed"});
        let resp: GithubPrResponse = self.api_patch(&url, body)?;
        Ok(convert_pr(resp))
    }
}

fn convert_pr(r: GithubPrResponse) -> PullRequest {
    PullRequest {
        number: r.number,
        html_url: r.html_url,
        state: match r.state.as_str() {
            "open" => PrState::Open,
            "closed" if r.merged_at.is_some() => PrState::Merged,
            "closed" => PrState::Closed,
            _ => PrState::Open,
        },
        draft: r.draft.unwrap_or(false),
        head: r.head.ref_field,
        base: r.base.ref_field,
    }
}

#[derive(Deserialize)]
struct GithubPrResponse {
    number: u64,
    html_url: String,
    state: String,
    draft: Option<bool>,
    merged_at: Option<String>,
    head: GithubBranchRef,
    base: GithubBranchRef,
}

#[derive(Deserialize)]
struct GithubBranchRef {
    #[serde(rename = "ref")]
    ref_field: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_open_pr() {
        let resp = GithubPrResponse {
            number: 42,
            html_url: "https://github.com/owner/repo/pull/42".into(),
            state: "open".into(),
            draft: Some(false),
            merged_at: None,
            head: GithubBranchRef { ref_field: "head-branch".into() },
            base: GithubBranchRef { ref_field: "base-branch".into() },
        };
        let pr = convert_pr(resp);
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, PrState::Open);
        assert!(!pr.draft);
    }

    #[test]
    fn convert_merged_pr() {
        let resp = GithubPrResponse {
            number: 42,
            html_url: "https://github.com/owner/repo/pull/42".into(),
            state: "closed".into(),
            draft: Some(false),
            merged_at: Some("2024-01-01T00:00:00Z".into()),
            head: GithubBranchRef { ref_field: "head".into() },
            base: GithubBranchRef { ref_field: "base".into() },
        };
        let pr = convert_pr(resp);
        assert_eq!(pr.state, PrState::Merged);
    }

    #[test]
    fn convert_closed_pr() {
        let resp = GithubPrResponse {
            number: 42,
            html_url: "https://github.com/owner/repo/pull/42".into(),
            state: "closed".into(),
            draft: Some(false),
            merged_at: None,
            head: GithubBranchRef { ref_field: "head".into() },
            base: GithubBranchRef { ref_field: "base".into() },
        };
        let pr = convert_pr(resp);
        assert_eq!(pr.state, PrState::Closed);
    }

    #[test]
    fn convert_draft_pr() {
        let resp = GithubPrResponse {
            number: 42,
            html_url: "https://github.com/owner/repo/pull/42".into(),
            state: "open".into(),
            draft: Some(true),
            merged_at: None,
            head: GithubBranchRef { ref_field: "head".into() },
            base: GithubBranchRef { ref_field: "base".into() },
        };
        let pr = convert_pr(resp);
        assert!(pr.draft);
    }
}
