//! Platform detection: pick the right [`PrEnricher`] for the current
//! repo + environment.
//!
//! Decision tree:
//!
//! 1. If `disabled == true`, return [`NoOpEnricher`].
//! 2. If the repo's `origin` (or `upstream`) remote points at GitHub,
//!    construct a [`GitHubEnricher`]. The token comes from the env
//!    (`GITHUB_TOKEN` / `GH_TOKEN`); anonymous mode is allowed for
//!    public repos but rate-limited.
//! 3. Otherwise, [`NoOpEnricher`] with a friendly stderr line so the
//!    user knows enrichment was skipped (not silently failing).

use std::path::Path;
use std::sync::Arc;

use crate::constants::GITHUB_TOKEN_ENV_VARS;
use crate::enrichment::github::GitHubEnricher;
use crate::enrichment::noop::NoOpEnricher;
use crate::enrichment::remote::detect_github_repo;
use crate::enrichment::{EnrichError, PrEnricher};
use crate::env::Env;

/// Choose an enricher for the given repo + env. Returns the chosen
/// enricher plus a one-line description of what was decided, suitable
/// for logging.
pub async fn detect(
    repo_root: &Path,
    env: &Env,
    disabled: bool,
) -> Result<(Arc<dyn PrEnricher>, String), EnrichError> {
    if disabled {
        return Ok((
            Arc::new(NoOpEnricher) as Arc<dyn PrEnricher>,
            "PR enrichment disabled by --no-pr-enrichment".to_string(),
        ));
    }

    match detect_github_repo(repo_root).await {
        Some(repo) => {
            let token = first_set_env(env, GITHUB_TOKEN_ENV_VARS);
            let token_note = match &token {
                Some(_) => "authenticated",
                None => "anonymous (60 req/hour limit)",
            };
            let info = format!(
                "PR enrichment via GitHub for {}/{} ({})",
                repo.owner, repo.repo, token_note
            );
            let enricher = GitHubEnricher::new(repo, token)?;
            Ok((Arc::new(enricher) as Arc<dyn PrEnricher>, info))
        }
        None => Ok((
            Arc::new(NoOpEnricher) as Arc<dyn PrEnricher>,
            "PR enrichment skipped (no GitHub remote detected)".to_string(),
        )),
    }
}

fn first_set_env(env: &Env, names: &[&str]) -> Option<String> {
    for n in names {
        if let Ok(v) = env.var(n)
            && !v.trim().is_empty()
        {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_returns_no_op() {
        let env = Env::mock(Vec::<(&str, &str)>::new());
        let (e, info) = detect(Path::new("."), &env, true).await.unwrap();
        assert_eq!(e.name(), "no-op");
        assert!(info.contains("disabled"));
    }

    #[test]
    fn first_set_env_picks_first_present() {
        let env = Env::mock([("GH_TOKEN", "gh"), ("GITHUB_TOKEN", "")]);
        // GITHUB_TOKEN is empty so we fall through to GH_TOKEN.
        let token = first_set_env(&env, &["GITHUB_TOKEN", "GH_TOKEN"]);
        assert_eq!(token.as_deref(), Some("gh"));
    }

    #[test]
    fn first_set_env_none_when_all_empty() {
        let env = Env::mock(Vec::<(&str, &str)>::new());
        assert!(first_set_env(&env, &["GITHUB_TOKEN", "GH_TOKEN"]).is_none());
    }
}
