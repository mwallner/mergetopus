use anyhow::{Result, bail, Context};
use base64::{Engine, engine::general_purpose::STANDARD as base64_engine};
use serde::Deserialize;

use super::{Forge, ForgeId, PrParams, PrUpdate, PullRequest, PrState};

type BbResponse = ureq::http::Response<ureq::Body>;

pub struct Bitbucket {
    token: String,
    host: String,
}

impl Bitbucket {
    pub fn new(token: String, host: String) -> Self {
        Bitbucket { token, host }
    }

    fn api_url(&self, path: &str) -> String {
        format!("https://{}/rest/api/latest{path}", self.host)
    }

    /// Web UI URL for a pull request on Bitbucket Data Center.
    fn pr_web_url(&self, project: &str, repo: &str, id: u64) -> String {
        format!("https://{}/projects/{project}/repos/{repo}/pull-requests/{id}", self.host)
    }

    fn agent(&self) -> ureq::Agent {
        ureq::agent()
    }

    fn headers<B>(&self, req: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        let auth = base64_engine.encode(format!(":{}", self.token));
        req.header("Authorization", &format!("Basic {auth}"))
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

    fn handle_response<T: serde::de::DeserializeOwned>(mut resp: BbResponse) -> Result<T> {
        let status = resp.status().as_u16();
        match status {
            200 | 201 => {
                let body = resp.body_mut();
                Ok(body.read_json::<T>()?)
            }
            204 => Ok(serde_json::from_value(serde_json::json!({}))?),
            401 => bail!("Bitbucket API: authentication failed; check your token"),
            403 => bail!("Bitbucket API: forbidden"),
            404 => bail!("Bitbucket API: resource not found (check project/repo)"),
            409 => bail!("Bitbucket API: conflict (version mismatch; re-fetch and retry)"),
            429 => bail!("Bitbucket API: rate limited"),
            s => {
                let body_str = resp.body_mut().read_to_string().unwrap_or_default();
                bail!("Bitbucket API: HTTP {s}: {body_str}")
            }
        }
    }

    fn ref_path(branch: &str) -> String {
        if branch.starts_with("refs/") {
            branch.to_string()
        } else {
            format!("refs/heads/{branch}")
        }
    }

    /// Fetch the current version of a PR for optimistic locking.
    fn fetch_version(&self, project: &str, repo: &str, number: u64) -> Result<i64> {
        let url = self.api_url(&format!(
            "/projects/{project}/repos/{repo}/pull-requests/{number}"
        ));
        let resp: BitbucketPrResponse = self.api_get(&url)?;
        Ok(resp.version)
    }
}

impl Forge for Bitbucket {
    fn id(&self) -> ForgeId {
        ForgeId::Bitbucket
    }

    fn name(&self) -> &str {
        "Bitbucket"
    }

    fn base_url(&self) -> &str {
        &self.host
    }

    fn create_pr(&self, params: PrParams) -> Result<PullRequest> {
        let url = self.api_url(&format!(
            "/projects/{}/repos/{}/pull-requests",
            params.owner, params.repo
        ));

        let mut body = serde_json::json!({
            "title": params.title,
            "fromRef": {
                "id": Self::ref_path(&params.head),
            },
            "toRef": {
                "id": Self::ref_path(&params.base),
            },
        });

        if !params.body.is_empty() {
            body["description"] = serde_json::json!(params.body);
        }

        let resp: BitbucketPrResponse = self.api_post(&url, body)?;
        let web_url = self.pr_web_url(&params.owner, &params.repo, resp.id);
        Ok(convert_pr(resp, web_url))
    }

    fn update_pr(&self, repo_path: &str, number: u64, params: PrUpdate) -> Result<PullRequest> {
        let Some((project, repo)) = repo_path.split_once('/') else {
            bail!("Bitbucket repo_path must be in project/repo format: {repo_path}");
        };

        let version = self.fetch_version(project, repo, number)?;
        let new_version = version + 1;

        let url = self.api_url(&format!(
            "/projects/{project}/repos/{repo}/pull-requests/{number}"
        ));

        let mut body = serde_json::json!({
            "version": new_version,
        });
        if let Some(title) = params.title {
            body["title"] = serde_json::json!(title);
        }
        if let Some(body_text) = params.body {
            body["description"] = serde_json::json!(body_text);
        }

        let resp: BitbucketPrResponse = self.api_put(&url, body)?;
        let web_url = self.pr_web_url(project, repo, resp.id);
        Ok(convert_pr(resp, web_url))
    }

    fn find_pr_by_head(&self, repo_path: &str, head: &str) -> Result<Option<PullRequest>> {
        let Some((project, repo)) = repo_path.split_once('/') else {
            bail!("Bitbucket repo_path must be in project/repo format: {repo_path}");
        };

        let at = Self::ref_path(head);
        let url = self.api_url(&format!(
            "/projects/{project}/repos/{repo}/pull-requests?at={}&state=OPEN",
            crate::forges::url_encode(&at)
        ));

        let resp: BitbucketPrListResponse = self.api_get(&url)?;
        Ok(resp.values.into_iter().next().map(|r| {
            let web_url = self.pr_web_url(project, repo, r.id);
            convert_pr(r, web_url)
        }))
    }

    fn close_pr(&self, repo_path: &str, number: u64) -> Result<PullRequest> {
        let Some((project, repo)) = repo_path.split_once('/') else {
            bail!("Bitbucket repo_path must be in project/repo format: {repo_path}");
        };

        let version = self.fetch_version(project, repo, number)?;
        let url = self.api_url(&format!(
            "/projects/{project}/repos/{repo}/pull-requests/{number}"
        ));
        let body = serde_json::json!({
            "version": version + 1,
            "state": "DECLINED",
        });
        let resp: BitbucketPrResponse = self.api_put(&url, body)?;
        let web_url = self.pr_web_url(project, repo, resp.id);
        Ok(convert_pr(resp, web_url))
    }
}

fn convert_pr(r: BitbucketPrResponse, web_url: String) -> PullRequest {
    PullRequest {
        number: r.id,
        html_url: web_url,
        state: match r.state.as_str() {
            "OPEN" => PrState::Open,
            "MERGED" => PrState::Merged,
            "DECLINED" | "DELETED" => PrState::Closed,
            _ => PrState::Open,
        },
        draft: false,
        head: r.from_ref.id,
        base: r.to_ref.id,
    }
}

#[derive(Deserialize)]
struct BitbucketPrResponse {
    id: u64,
    version: i64,
    state: String,
    from_ref: BitbucketRef,
    to_ref: BitbucketRef,
}

#[derive(Deserialize)]
struct BitbucketRef {
    id: String,
}

#[derive(Deserialize)]
struct BitbucketPrListResponse {
    values: Vec<BitbucketPrResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_response(id: u64, state: &str) -> BitbucketPrResponse {
        BitbucketPrResponse {
            id,
            version: 1,
            state: state.into(),
            from_ref: BitbucketRef { id: "refs/heads/feature".into() },
            to_ref: BitbucketRef { id: "refs/heads/main".into() },
        }
    }

    #[test]
    fn convert_open_pr() {
        let resp = make_response(42, "OPEN");
        let pr = convert_pr(resp, "https://bb.example.com/projects/PROJ/repos/repo/pull-requests/42".into());
        assert_eq!(pr.number, 42);
        assert_eq!(pr.html_url, "https://bb.example.com/projects/PROJ/repos/repo/pull-requests/42");
        assert_eq!(pr.state, PrState::Open);
        assert!(!pr.draft);
    }

    #[test]
    fn convert_merged_pr() {
        let resp = make_response(42, "MERGED");
        let pr = convert_pr(resp, "https://bb.example.com/projects/PROJ/repos/repo/pull-requests/42".into());
        assert_eq!(pr.state, PrState::Merged);
    }

    #[test]
    fn convert_declined_pr() {
        let resp = make_response(42, "DECLINED");
        let pr = convert_pr(resp, "https://bb.example.com/projects/PROJ/repos/repo/pull-requests/42".into());
        assert_eq!(pr.state, PrState::Closed);
    }

    #[test]
    fn ref_path_adds_prefix() {
        assert_eq!(Bitbucket::ref_path("feature"), "refs/heads/feature");
    }

    #[test]
    fn ref_path_preserves_full() {
        assert_eq!(
            Bitbucket::ref_path("refs/heads/feature"),
            "refs/heads/feature"
        );
    }
}
