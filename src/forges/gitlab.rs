use anyhow::{Result, bail, Context};
use serde::Deserialize;

use super::{Forge, ForgeId, PrParams, PrUpdate, PullRequest, PrState};

type GlResponse = ureq::http::Response<ureq::Body>;

pub struct GitLab {
    token: String,
    host: String,
}

impl GitLab {
    pub fn new(token: String, host: String) -> Self {
        GitLab { token, host }
    }

    fn api_url(&self, path: &str) -> String {
        format!("https://{}/api/v4{path}", self.host)
    }

    fn encode_project_path(repo_path: &str) -> String {
        repo_path.replace('/', "%2F")
    }

    fn agent(&self) -> ureq::Agent {
        ureq::agent()
    }

    fn headers<B>(&self, req: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        req.header("PRIVATE-TOKEN", &self.token)
            .header("Accept", "application/json")
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

    fn api_put<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        body: serde_json::Value,
    ) -> Result<T> {
        let resp = self
            .headers(self.agent().put(url))
            .send_json(body)
            .with_context(|| format!("PUT {url} failed"))?;
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

    fn handle_response<T: serde::de::DeserializeOwned>(mut resp: GlResponse) -> Result<T> {
        let status = resp.status().as_u16();
        match status {
            200 | 201 => {
                let body = resp.body_mut();
                Ok(body.read_json::<T>()?)
            }
            204 => Ok(serde_json::from_value(serde_json::json!({}))?),
            401 => bail!("GitLab API: authentication failed; check your token"),
            403 => bail!("GitLab API: forbidden or insufficient scope"),
            404 => bail!("GitLab API: resource not found (check project path)"),
            429 => bail!("GitLab API: rate limited"),
            422 => {
                let body_str = resp.body_mut().read_to_string().unwrap_or_default();
                bail!("GitLab API: validation error (HTTP 422): {body_str}")
            }
            s => {
                let body_str = resp.body_mut().read_to_string().unwrap_or_default();
                bail!("GitLab API: HTTP {s}: {body_str}")
            }
        }
    }
}

impl Forge for GitLab {
    fn id(&self) -> ForgeId {
        ForgeId::GitLab
    }

    fn name(&self) -> &str {
        "GitLab"
    }

    fn base_url(&self) -> &str {
        &self.host
    }

    fn create_pr(&self, params: PrParams) -> Result<PullRequest> {
        let project = if params.owner.is_empty() {
            Self::encode_project_path(&params.repo)
        } else {
            Self::encode_project_path(&format!("{}/{}", params.owner, params.repo))
        };
        let url = self.api_url(&format!("/projects/{project}/merge_requests"));

        let mut body = serde_json::json!({
            "source_branch": params.head,
            "target_branch": params.base,
            "title": params.title,
            "draft": params.draft,
        });

        if !params.body.is_empty() {
            body["description"] = serde_json::json!(params.body);
        }

        let resp: GitlabMrResponse = self.api_post(&url, body)?;
        Ok(convert_mr(resp))
    }

    fn update_pr(&self, repo_path: &str, number: u64, params: PrUpdate) -> Result<PullRequest> {
        let project = Self::encode_project_path(repo_path);
        let url = self.api_url(&format!("/projects/{project}/merge_requests/{number}"));

        let mut body = serde_json::json!({});
        if let Some(title) = params.title {
            body["title"] = serde_json::json!(title);
        }
        if let Some(body_text) = params.body {
            body["description"] = serde_json::json!(body_text);
        }

        let resp: GitlabMrResponse = self.api_put(&url, body)?;
        Ok(convert_mr(resp))
    }

    fn find_pr_by_head(&self, repo_path: &str, head: &str) -> Result<Option<PullRequest>> {
        let project = Self::encode_project_path(repo_path);
        let url = self.api_url(&format!(
            "/projects/{project}/merge_requests?source_branch={}&state=opened",
            crate::forges::url_encode(head)
        ));

        let resp: Vec<GitlabMrResponse> = self.api_get(&url)?;
        Ok(resp.into_iter().next().map(convert_mr))
    }

    fn close_pr(&self, repo_path: &str, number: u64) -> Result<PullRequest> {
        let project = Self::encode_project_path(repo_path);
        let url = self.api_url(&format!("/projects/{project}/merge_requests/{number}"));
        let body = serde_json::json!({"state_event": "close"});
        let resp: GitlabMrResponse = self.api_put(&url, body)?;
        Ok(convert_mr(resp))
    }
}

fn convert_mr(r: GitlabMrResponse) -> PullRequest {
    PullRequest {
        number: r.iid,
        html_url: r.web_url,
        state: match r.state.as_str() {
            "opened" => PrState::Open,
            "merged" => PrState::Merged,
            "closed" => PrState::Closed,
            _ => PrState::Open,
        },
        draft: r.draft,
        head: r.source_branch,
        base: r.target_branch,
    }
}

#[derive(Deserialize)]
struct GitlabMrResponse {
    iid: u64,
    web_url: String,
    state: String,
    draft: bool,
    source_branch: String,
    target_branch: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_open_mr() {
        let resp = GitlabMrResponse {
            iid: 7,
            web_url: "https://gitlab.com/owner/repo/-/merge_requests/7".into(),
            state: "opened".into(),
            draft: false,
            source_branch: "feature".into(),
            target_branch: "main".into(),
        };
        let pr = convert_mr(resp);
        assert_eq!(pr.number, 7);
        assert_eq!(pr.state, PrState::Open);
        assert!(!pr.draft);
        assert_eq!(pr.head, "feature");
        assert_eq!(pr.base, "main");
    }

    #[test]
    fn convert_merged_mr() {
        let resp = GitlabMrResponse {
            iid: 7,
            web_url: "https://gitlab.com/owner/repo/-/merge_requests/7".into(),
            state: "merged".into(),
            draft: false,
            source_branch: "feature".into(),
            target_branch: "main".into(),
        };
        let pr = convert_mr(resp);
        assert_eq!(pr.state, PrState::Merged);
    }

    #[test]
    fn convert_closed_mr() {
        let resp = GitlabMrResponse {
            iid: 7,
            web_url: "https://gitlab.com/owner/repo/-/merge_requests/7".into(),
            state: "closed".into(),
            draft: false,
            source_branch: "feature".into(),
            target_branch: "main".into(),
        };
        let pr = convert_mr(resp);
        assert_eq!(pr.state, PrState::Closed);
    }

    #[test]
    fn convert_draft_mr() {
        let resp = GitlabMrResponse {
            iid: 7,
            web_url: "https://gitlab.com/owner/repo/-/merge_requests/7".into(),
            state: "opened".into(),
            draft: true,
            source_branch: "feature".into(),
            target_branch: "main".into(),
        };
        let pr = convert_mr(resp);
        assert!(pr.draft);
    }

    #[test]
    fn encode_project_path_encodes_slashes() {
        assert_eq!(
            GitLab::encode_project_path("group/subgroup/project"),
            "group%2Fsubgroup%2Fproject"
        );
    }

    #[test]
    fn encode_project_path_preserves_simple() {
        assert_eq!(
            GitLab::encode_project_path("owner/repo"),
            "owner%2Frepo"
        );
    }
}
