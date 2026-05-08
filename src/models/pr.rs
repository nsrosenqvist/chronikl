//! Pull-request enrichment data attached to a [`Commit`](super::Commit).
//!
//! Populated by the enrichment layer after `git log` and before the
//! ladder runs. Platform-agnostic; chronikl currently only fetches from
//! GitHub but the schema would suit GitLab MRs and Bitbucket PRs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrInfo {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub author: Option<String>,
    pub merged_at: Option<String>,
    pub url: Option<String>,
}
