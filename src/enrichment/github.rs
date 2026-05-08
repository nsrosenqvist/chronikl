//! GitHub PR enrichment via octocrab.
//!
//! For each commit, calls `GET /repos/{owner}/{repo}/commits/{sha}/pulls`
//! and attaches the first matching PR to the commit. Failures on a
//! single commit do not abort the pass — the commit just stays without
//! `pr` data.
//!
//! Public-repo calls work without a token (subject to a 60-req/hour
//! anonymous rate limit). With a token (`GITHUB_TOKEN` / `GH_TOKEN`,
//! standard in GitHub Actions) the limit is 5000/hour.

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use octocrab::Octocrab;
use serde::Deserialize;

use crate::constants::PR_CONCURRENCY;
use crate::enrichment::remote::GitHubRepo;
use crate::enrichment::{EnrichError, EnrichOutcome, PrEnricher};
use crate::models::{EnrichedCommit, PrInfo};

#[derive(Debug)]
pub struct GitHubEnricher {
    repo: GitHubRepo,
    client: Arc<Octocrab>,
}

impl GitHubEnricher {
    /// Construct a GitHub enricher. `token` is optional — anonymous
    /// access works for public repos but is rate-limited to 60/hour.
    pub fn new(repo: GitHubRepo, token: Option<String>) -> Result<Self, EnrichError> {
        ensure_crypto_provider();
        let mut builder = octocrab::OctocrabBuilder::new();
        if let Some(t) = token {
            builder = builder.personal_token(t);
        }
        let client = builder
            .build()
            .map_err(|e| EnrichError::NotConfigured(format!("octocrab build failed: {e}")))?;
        Ok(Self {
            repo,
            client: Arc::new(client),
        })
    }
}

/// Install rustls' aws-lc-rs crypto provider as the process default,
/// once. octocrab 0.46 pulls in rustls 0.23 without a unique
/// crypto-provider feature, which makes rustls panic on first TLS use
/// because it can't auto-select between aws-lc-rs and ring. We install
/// aws-lc-rs explicitly here. Idempotent and a no-op if a provider is
/// already installed.
fn ensure_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Minimal PR shape we deserialize from
/// `GET /repos/{owner}/{repo}/commits/{sha}/pulls`. We avoid pulling
/// in octocrab's full `PullRequest` struct so that schema changes in
/// the GitHub API don't break us — we only consume the fields we use.
#[derive(Debug, Deserialize)]
struct AssociatedPr {
    number: u64,
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    user: Option<UserShape>,
    #[serde(default)]
    labels: Vec<LabelShape>,
    #[serde(default)]
    merged_at: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UserShape {
    login: String,
}

#[derive(Debug, Deserialize)]
struct LabelShape {
    name: String,
}

#[async_trait]
impl PrEnricher for GitHubEnricher {
    async fn enrich(&self, commits: &mut [EnrichedCommit]) -> Result<EnrichOutcome, EnrichError> {
        // Materialize (idx, sha, short_sha) up front so the futures own
        // everything they need and don't borrow `commits`. Each fetch
        // is independent (bounded by PR_CONCURRENCY); failures are
        // logged and skipped so a single 404/timeout doesn't abort the
        // whole pass.
        let inputs: Vec<(usize, String, String)> = commits
            .iter()
            .enumerate()
            .map(|(idx, entry)| {
                (
                    idx,
                    entry.commit.sha.clone(),
                    entry.commit.short_sha.clone(),
                )
            })
            .collect();

        let fetches = inputs.into_iter().map(|(idx, sha, short_sha)| {
            let client = self.client.clone();
            let repo = self.repo.clone();
            async move {
                let result = fetch_associated_pr(&client, &repo, &sha).await;
                (idx, short_sha, result)
            }
        });

        let outcomes: Vec<(usize, String, Result<Option<PrInfo>, EnrichError>)> =
            stream::iter(fetches)
                .buffer_unordered(PR_CONCURRENCY)
                .collect()
                .await;

        let mut enriched = 0usize;
        let mut failed = 0usize;
        let mut first_error: Option<String> = None;
        for (idx, short_sha, result) in outcomes {
            match result {
                Ok(Some(pr)) => {
                    commits[idx].pr = Some(pr);
                    enriched += 1;
                }
                Ok(None) => {}
                Err(err) => {
                    failed += 1;
                    if first_error.is_none() {
                        first_error = Some(format!("{short_sha}: {err}"));
                    }
                }
            }
        }
        Ok(EnrichOutcome {
            enriched,
            failed,
            first_error,
        })
    }

    fn name(&self) -> &str {
        "github"
    }
}

async fn fetch_associated_pr(
    client: &Octocrab,
    repo: &GitHubRepo,
    sha: &str,
) -> Result<Option<PrInfo>, EnrichError> {
    let path = format!(
        "/repos/{owner}/{name}/commits/{sha}/pulls",
        owner = repo.owner,
        name = repo.repo,
    );
    // Some commits have many associated PRs (e.g. base-branch
    // backports). We keep the first one — typically the originating PR.
    let prs: Vec<AssociatedPr> = client
        .get(path, None::<&()>)
        .await
        .map_err(|e| EnrichError::Api(format!("{e}")))?;
    Ok(prs.into_iter().next().map(|p| PrInfo {
        number: p.number,
        title: p.title,
        body: p.body.unwrap_or_default(),
        labels: p.labels.into_iter().map(|l| l.name).collect(),
        author: p.user.map(|u| u.login),
        merged_at: p.merged_at,
        url: p.html_url,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn constructs_with_token() {
        let repo = GitHubRepo {
            owner: "foo".into(),
            repo: "bar".into(),
        };
        assert!(GitHubEnricher::new(repo, Some("ghp_test".into())).is_ok());
    }

    #[tokio::test]
    async fn constructs_without_token() {
        let repo = GitHubRepo {
            owner: "foo".into(),
            repo: "bar".into(),
        };
        // Anonymous client should still build.
        assert!(GitHubEnricher::new(repo, None).is_ok());
    }
}
