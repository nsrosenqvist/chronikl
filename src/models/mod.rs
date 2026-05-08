//! Shared data types passed between modules.
//!
//! # Bounded Context: Domain Models
//!
//! All inter-module communication goes through the types defined here.
//! Keeping them in one place makes it easy to reason about what data
//! flows through the pipeline.

mod classification;
mod pr;
mod release;
mod tokens;

pub use classification::{
    Classification, ClassificationSource, Classified, ClassifiedCommit, Section,
};
pub use pr::PrInfo;

/// A [`Commit`] paired with PR-platform metadata fetched during the
/// enrichment pass. Keeping `PrInfo` here (rather than on [`Commit`])
/// preserves the boundary between the git domain (commits as recorded
/// in history) and the PR-platform domain (whatever GitHub/GitLab/…
/// adds on top).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrichedCommit {
    pub commit: Commit,
    /// `None` when no PR could be associated, the platform isn't
    /// supported, or enrichment was skipped.
    #[serde(default)]
    pub pr: Option<PrInfo>,
}

impl EnrichedCommit {
    pub fn from_commit(commit: Commit) -> Self {
        Self { commit, pr: None }
    }

    pub fn from_commits(commits: Vec<Commit>) -> Vec<Self> {
        commits.into_iter().map(Self::from_commit).collect()
    }
}
pub use release::{ParsedVersion, ReleaseKind, VersionBump, VersionScheme};
pub use tokens::TokenUsage;

use serde::{Deserialize, Serialize};
use strum::Display;

/// Conventional Commits classification parsed from a commit subject.
///
/// Spec: <https://www.conventionalcommits.org/>
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conventional {
    /// e.g. `feat`, `fix`, `chore`, `docs`, `refactor`, …
    pub kind: String,
    /// Optional scope — `feat(api): …` → `Some("api")`.
    pub scope: Option<String>,
    /// Marked breaking via `!` in the subject (e.g. `feat!: …`) or via a
    /// `BREAKING CHANGE:` footer in the body.
    pub breaking: bool,
    /// The descriptive part after the colon.
    pub description: String,
}

/// Single commit, plus enrichment derived from parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub sha: String,
    pub short_sha: String,
    pub author_name: String,
    pub author_email: String,
    /// Author date as ISO 8601 (`%aI`).
    pub author_date: String,
    /// Parent SHAs. A merge commit has two or more parents.
    pub parents: Vec<String>,
    pub subject: String,
    pub body: String,
    pub files: Vec<String>,
    /// Parsed from squash-merge subjects like `... (#123)`.
    pub pr_id: Option<u64>,
    /// Conventional Commits parse, if the subject matches.
    pub conventional: Option<Conventional>,
    /// True if `!` marker on the type or a `BREAKING CHANGE:` footer is present.
    pub breaking: bool,
}

impl Commit {
    /// Whether this commit has more than one parent (i.e. is a merge commit).
    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }
}

/// Detected style of a commit range — drives how aggressively the ladder
/// should run on commit messages vs. fall back to diff-aware classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum MergeStyle {
    /// Most commits look like `<PR title> (#NNN)` — squash-merge produces
    /// curated subjects, so the LLM mostly needs to polish prose.
    Squash,
    /// Many `Merge pull request #NNN …` commits — original commits remain
    /// alongside the merge commit, so messages are usually raw.
    Merge,
    /// Single-parent commits without PR-ID suffixes — raw messages, the
    /// LLM has to do the most work.
    Rebase,
    /// Mixed signals; treat as Rebase for safety.
    Mixed,
}
