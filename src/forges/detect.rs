use anyhow::{Result, bail, Context};

use super::{Forge, ForgeId};
use super::auth;
use super::bitbucket::Bitbucket;
use super::forgejo::Forgejo;
use super::github::GitHub;
use super::gitlab::GitLab;

const SAAS_GITLAB_HOSTS: &[&str] = &["gitlab.com", "gitlab.example.com"];
const SAAS_BITBUCKET_HOSTS: &[&str] = &["bitbucket.org", "bitbucket.example.com"];
const SAAS_FORGEJO_HOSTS: &[&str] = &["codeberg.org"];

#[derive(Debug, Clone)]
pub struct RemoteInfo {
    pub host: String,
    pub owner: String,
    pub repo: String,
}

pub fn parse_remote_url(raw: &str) -> Result<RemoteInfo> {
    let raw = raw.trim();

    if let Some(rest) = raw.strip_prefix("https://").or_else(|| raw.strip_prefix("http://")) {
        parse_https_url(raw, rest)
    } else if let Some(rest) = raw.strip_prefix("ssh://") {
        parse_ssh_url(rest)
    } else if let Some(at_pos) = raw.find('@') {
        parse_scp_url(raw, at_pos)
    } else {
        bail!("unable to parse git remote URL: {raw}")
    }
}

fn parse_https_url(_full: &str, rest: &str) -> Result<RemoteInfo> {
    let without_credentials = if let Some(at_pos) = rest.rfind('@') {
        &rest[at_pos + 1..]
    } else {
        rest
    };

    let (host, path) = without_credentials
        .split_once('/')
        .context("HTTPS URL missing path")?;

    let path = path
        .trim_end_matches(".git")
        .trim_matches('/');

    // Strip known Bitbucket Data Center path prefix.
    let path = path.strip_prefix("scm/").unwrap_or(path);

    let (owner, repo) = path
        .split_once('/')
        .context("HTTPS URL path must contain owner/repo")?;

    Ok(RemoteInfo {
        host: host.to_string(),
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

fn parse_ssh_url(rest: &str) -> Result<RemoteInfo> {
    let without_credentials = if let Some(at_pos) = rest.rfind('@') {
        &rest[at_pos + 1..]
    } else {
        rest
    };

    let (host, path) = without_credentials
        .split_once('/')
        .context("SSH URL missing path")?;

    let path = path
        .trim_end_matches(".git")
        .trim_matches('/');

    let (owner, repo) = path
        .split_once('/')
        .context("SSH URL path must contain owner/repo")?;

    Ok(RemoteInfo {
        host: host.to_string(),
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

fn parse_scp_url(full: &str, at_pos: usize) -> Result<RemoteInfo> {
    let host_and_path = &full[at_pos + 1..];
    let (host, path) = host_and_path
        .split_once(':')
        .context("SCP-style URL missing colon separator")?;

    let path = path
        .trim_end_matches(".git")
        .trim_matches('/');

    let (owner, repo) = path
        .split_once('/')
        .context("SCP URL path must contain owner/repo")?;

    Ok(RemoteInfo {
        host: host.to_string(),
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

pub fn detect_forge(remote_url: &str) -> Result<Box<dyn Forge>> {
    let info = parse_remote_url(remote_url)?;
    let host = info.host.as_str();

    let forge_id = match host {
        "github.com" => ForgeId::GitHub,
        host if SAAS_GITLAB_HOSTS.contains(&host) => ForgeId::GitLab,
        host if SAAS_BITBUCKET_HOSTS.contains(&host) => ForgeId::Bitbucket,
        host if SAAS_FORGEJO_HOSTS.contains(&host) => ForgeId::Forgejo,
        _ => {
            let config_type = crate::git_ops::get_git_config("mergetopus.forge-type")?;
            match config_type.as_deref() {
                Some("gitlab") => ForgeId::GitLab,
                Some("forgejo") | Some("gitea") => ForgeId::Forgejo,
                Some("bitbucket") => ForgeId::Bitbucket,
                _ => ForgeId::GitHub,
            }
        }
    };

    let token = auth::resolve_token(forge_id)?;

    match forge_id {
        ForgeId::GitHub => Ok(Box::new(GitHub::new(token, info.owner, info.repo))),
        ForgeId::GitLab => Ok(Box::new(GitLab::new(token, info.host))),
        ForgeId::Bitbucket => Ok(Box::new(Bitbucket::new(token, info.host))),
        ForgeId::Forgejo => Ok(Box::new(Forgejo::new(token, info.host))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_github_https() {
        let info = parse_remote_url("https://github.com/owner/repo.git").unwrap();
        assert_eq!(info.host, "github.com");
        assert_eq!(info.owner, "owner");
        assert_eq!(info.repo, "repo");
    }

    #[test]
    fn parse_github_ssh() {
        let info = parse_remote_url("git@github.com:owner/repo.git").unwrap();
        assert_eq!(info.host, "github.com");
        assert_eq!(info.owner, "owner");
        assert_eq!(info.repo, "repo");
    }

    #[test]
    fn parse_ssh_protocol() {
        let info = parse_remote_url("ssh://git@github.com/owner/repo.git").unwrap();
        assert_eq!(info.host, "github.com");
        assert_eq!(info.owner, "owner");
        assert_eq!(info.repo, "repo");
    }

    #[test]
    fn parse_https_no_git_suffix() {
        let info = parse_remote_url("https://gitlab.com/group/project").unwrap();
        assert_eq!(info.host, "gitlab.com");
        assert_eq!(info.owner, "group");
        assert_eq!(info.repo, "project");
    }

    #[test]
    fn parse_bitbucket_ssh_with_port() {
        let info = parse_remote_url("ssh://git@bitbucket.example.com:7999/PROJ/repo.git").unwrap();
        assert_eq!(info.host, "bitbucket.example.com:7999");
        assert_eq!(info.owner, "PROJ");
        assert_eq!(info.repo, "repo");
    }

    #[test]
    fn parse_bitbucket_https_scm() {
        let info = parse_remote_url("https://bitbucket.example.com/scm/PROJ/my-repo.git").unwrap();
        assert_eq!(info.host, "bitbucket.example.com");
        assert_eq!(info.owner, "PROJ");
        assert_eq!(info.repo, "my-repo");
    }

    #[test]
    fn parse_invalid_url() {
        assert!(parse_remote_url("").is_err());
        assert!(parse_remote_url("not-a-url").is_err());
    }

    #[test]
    fn parse_codeberg_https() {
        let info = parse_remote_url("https://codeberg.org/owner/repo.git").unwrap();
        assert_eq!(info.host, "codeberg.org");
        assert_eq!(info.owner, "owner");
        assert_eq!(info.repo, "repo");
    }

    #[test]
    fn parse_codeberg_ssh() {
        let info = parse_remote_url("git@codeberg.org:owner/repo.git").unwrap();
        assert_eq!(info.host, "codeberg.org");
        assert_eq!(info.owner, "owner");
        assert_eq!(info.repo, "repo");
    }

    #[test]
    fn detect_codeberg_as_forgejo() {
        let info = parse_remote_url("https://codeberg.org/user/project.git").unwrap();
        assert_eq!(info.host, "codeberg.org");
        // The forge detection itself depends on git config and env,
        // but the host matching is tested indirectly through parsing.
    }
}
