use anyhow::{Result, bail, Context};
use serde::Deserialize;

use super::{Forge, ForgeId, PrParams, PrUpdate, PullRequest, PrState};

type FjResponse = ureq::http::Response<ureq::Body>;

pub struct Forgejo {
    token: String,
    host: String,
}

impl Forgejo {
    pub fn new(token: String, host: String) -> Self {
        Forgejo { token, host }
    }

    fn api_url(&self, path: &str) -> String {
        format!("https://{}/api/v1{path}", self.host)
    }

    fn agent(&self) -> ureq::Agent {
        ureq::agent()
    }

    fn headers<B>(&self, req: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        req.header("Authorization", &format!("Bearer {}", self.token))
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

    fn handle_response<T: serde::de::DeserializeOwned>(mut resp: FjResponse) -> Result<T> {
        let status = resp.status().as_u16();
        match status {
            200 | 201 => {
                let body = resp.body_mut();
                Ok(body.read_json::<T>()?)
            }
            204 => Ok(serde_json::from_value(serde_json::json!({}))?),
            401 => bail!("Forgejo API: authentication failed; check your token"),
            403 => bail!("Forgejo API: forbidden or insufficient scope"),
            404 => bail!("Forgejo API: resource not found (check owner/repo)"),
            409 => bail!("Forgejo API: conflict (PR already exists for this branch)"),
            422 => {
                let body_str = resp.body_mut().read_to_string().unwrap_or_default();
                bail!("Forgejo API: validation error (HTTP 422): {body_str}")
            }
            s => {
                let body_str = resp.body_mut().read_to_string().unwrap_or_default();
                bail!("Forgejo API: HTTP {s}: {body_str}")
            }
        }
    }
}

impl Forge for Forgejo {
    fn id(&self) -> ForgeId {
        ForgeId::Forgejo
    }

    fn name(&self) -> &str {
        "Forgejo"
    }

    fn base_url(&self) -> &str {
        &self.host
    }

    fn create_pr(&self, params: PrParams) -> Result<PullRequest> {
        let url = self.api_url(&format!(
            "/repos/{}/{}/pulls",
            params.owner, params.repo
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

        let resp: ForgejoPrResponse = self.api_post(&url, body)?;
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

        let resp: ForgejoPrResponse = self.api_patch(&url, body)?;
        Ok(convert_pr(resp))
    }

    fn find_pr_by_head(&self, repo_path: &str, head: &str) -> Result<Option<PullRequest>> {
        let url = self.api_url(&format!(
            "/repos/{repo_path}/pulls?head={}&state=open",
            crate::forges::url_encode(head)
        ));

        let resp: Vec<ForgejoPrResponse> = self.api_get(&url)?;
        Ok(resp.into_iter().next().map(convert_pr))
    }

    fn close_pr(&self, repo_path: &str, number: u64) -> Result<PullRequest> {
        let url = self.api_url(&format!("/repos/{repo_path}/pulls/{number}"));
        let body = serde_json::json!({"state": "closed"});
        let resp: ForgejoPrResponse = self.api_patch(&url, body)?;
        Ok(convert_pr(resp))
    }
}

fn convert_pr(r: ForgejoPrResponse) -> PullRequest {
    PullRequest {
        number: r.number,
        html_url: r.html_url,
        state: match r.state.as_str() {
            "open" => PrState::Open,
            "closed" if r.merged_at.is_some() => PrState::Merged,
            "closed" => PrState::Closed,
            _ => PrState::Open,
        },
        draft: r.draft,
        head: r.head.label,
        base: r.base.label,
    }
}

#[derive(Deserialize)]
struct ForgejoPrResponse {
    number: u64,
    html_url: String,
    state: String,
    draft: bool,
    merged_at: Option<String>,
    head: ForgejoBranchRef,
    base: ForgejoBranchRef,
}

#[derive(Deserialize)]
struct ForgejoBranchRef {
    label: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_response(number: u64, state: &str, merged_at: Option<&str>) -> ForgejoPrResponse {
        ForgejoPrResponse {
            number,
            html_url: format!("https://forgejo.example.org/owner/repo/pulls/{number}"),
            state: state.into(),
            draft: false,
            merged_at: merged_at.map(String::from),
            head: ForgejoBranchRef { label: "feature".into() },
            base: ForgejoBranchRef { label: "main".into() },
        }
    }

    #[test]
    fn convert_open_pr() {
        let resp = make_response(7, "open", None);
        let pr = convert_pr(resp);
        assert_eq!(pr.number, 7);
        assert_eq!(pr.state, PrState::Open);
        assert!(!pr.draft);
    }

    #[test]
    fn convert_merged_pr() {
        let resp = make_response(7, "closed", Some("2024-01-01T00:00:00Z"));
        let pr = convert_pr(resp);
        assert_eq!(pr.state, PrState::Merged);
    }

    #[test]
    fn convert_closed_pr() {
        let resp = make_response(7, "closed", None);
        let pr = convert_pr(resp);
        assert_eq!(pr.state, PrState::Closed);
    }

    #[test]
    fn convert_draft_pr() {
        let mut resp = make_response(7, "open", None);
        resp.draft = true;
        let pr = convert_pr(resp);
        assert!(pr.draft);
    }
}
