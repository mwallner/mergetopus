use anyhow::{Result, bail};

use crate::git_ops;
use super::ForgeId;

pub fn resolve_token(forge: ForgeId) -> Result<String> {
    let key = config_key(forge);
    if let Some(token) = git_ops::get_git_config(&key)? {
        let t = token.trim().to_string();
        if !t.is_empty() {
            return Ok(t);
        }
    }

    if let Some(token) = resolve_env(forge) {
        return Ok(token);
    }

    // Forgejo-specific fallback: also check CODEBERG_TOKEN.
    if forge == ForgeId::Forgejo {
        if let Ok(token) = std::env::var("CODEBERG_TOKEN") {
            let t = token.trim().to_string();
            if !t.is_empty() {
                return Ok(t);
            }
        }
    }

    let env_var = env_var_name(forge);
    bail!(
        "no authentication token found for {forge_name};\n\
         set git config `{key}` or environment variable `{env_var}`",
        forge_name = forge.name(),
        key = key,
        env_var = env_var,
    )
}

fn resolve_env(forge: ForgeId) -> Option<String> {
    let env_var = env_var_name(forge);
    if let Ok(token) = std::env::var(&env_var) {
        let t = token.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    None
}

fn config_key(forge: ForgeId) -> String {
    match forge {
        ForgeId::GitHub => "mergetopus.github-token".to_string(),
        ForgeId::GitLab => "mergetopus.gitlab-token".to_string(),
        ForgeId::Bitbucket => "mergetopus.bitbucket-token".to_string(),
        ForgeId::Forgejo => "mergetopus.forgejo-token".to_string(),
    }
}

fn env_var_name(forge: ForgeId) -> String {
    match forge {
        ForgeId::GitHub => "GITHUB_TOKEN".to_string(),
        ForgeId::GitLab => "GITLAB_TOKEN".to_string(),
        ForgeId::Bitbucket => "BITBUCKET_TOKEN".to_string(),
        ForgeId::Forgejo => "FORGEJO_TOKEN".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_key_format() {
        assert_eq!(config_key(ForgeId::GitHub), "mergetopus.github-token");
        assert_eq!(config_key(ForgeId::GitLab), "mergetopus.gitlab-token");
        assert_eq!(config_key(ForgeId::Bitbucket), "mergetopus.bitbucket-token");
        assert_eq!(config_key(ForgeId::Forgejo), "mergetopus.forgejo-token");
    }

    #[test]
    fn env_var_name_format() {
        assert_eq!(env_var_name(ForgeId::GitHub), "GITHUB_TOKEN");
        assert_eq!(env_var_name(ForgeId::GitLab), "GITLAB_TOKEN");
        assert_eq!(env_var_name(ForgeId::Bitbucket), "BITBUCKET_TOKEN");
        assert_eq!(env_var_name(ForgeId::Forgejo), "FORGEJO_TOKEN");
    }

    #[test]
    fn codeberg_token_fallback() {
        // CODEBERG_TOKEN is an additional fallback for Forgejo auth.
        // The main env var is FORGEJO_TOKEN; this tests the config key
        // still returns the primary key.
        assert_eq!(config_key(ForgeId::Forgejo), "mergetopus.forgejo-token");
    }
}
