//! Pull-request enrichment for commits.
//!
//! # Bounded Context: PR Enrichment
//!
//! Given a list of commits, attach PR metadata (title, body, labels,
//! author) when the underlying forge can supply it. Strategy-pattern:
//! [`PrEnricher`] is the trait, with implementations:
//!
//! - [`github::GitHubEnricher`] — uses octocrab against the public
//!   GitHub REST API. Automatically picked when running on GitHub
//!   Actions or when `GITHUB_TOKEN` / `GH_TOKEN` is set.
//! - [`noop::NoOpEnricher`] — returns the commits unchanged. Used on
//!   non-GitHub CIs (GitLab, Bitbucket, …) and when no token is
//!   available locally.
//!
//! Enrichment is opt-out: the user passes `--no-pr-enrichment` (or sets
//! `CHRONIKL_NO_PR_ENRICHMENT`) to force the no-op path.

pub mod github;
pub mod noop;
pub mod platform;
pub mod remote;

use async_trait::async_trait;
use thiserror::Error;

use crate::models::EnrichedCommit;

#[derive(Debug, Error)]
pub enum EnrichError {
    #[error("API error: {0}")]
    Api(String),
    #[error("not configured: {0}")]
    NotConfigured(String),
    #[error("other: {0}")]
    Other(String),
}

/// Result of an enrichment pass. Reports both the success count and
/// any per-commit failures so the caller can render an accurate stage
/// status (success / warn / partial).
#[derive(Debug, Default)]
pub struct EnrichOutcome {
    /// Commits that gained PR data this pass.
    pub enriched: usize,
    /// Commits whose lookup failed (network error, rate limit, …).
    /// Best-effort: a commit-level failure does not abort the pass.
    pub failed: usize,
    /// Representative error from the first failed lookup, formatted
    /// as `"<short_sha>: <message>"`. Lets the caller surface why
    /// without spamming one line per commit.
    pub first_error: Option<String>,
}

#[async_trait]
pub trait PrEnricher: Send + Sync {
    /// Enrich commits in place. Returns an [`EnrichOutcome`] describing
    /// what landed and what failed. Implementations should be
    /// best-effort: a commit-level lookup failure should not abort
    /// the whole pass.
    async fn enrich(&self, commits: &mut [EnrichedCommit]) -> Result<EnrichOutcome, EnrichError>;

    /// Short human-readable label for the audit log.
    fn name(&self) -> &str;
}

pub use github::GitHubEnricher;
pub use noop::NoOpEnricher;

// Re-export the URL-parsing helpers for convenience.
pub use remote::{Forge, GitHubRepo, detect_forge};
