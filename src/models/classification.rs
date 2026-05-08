//! Classification of commits into release-notes sections.

use serde::{Deserialize, Serialize};
use strum::Display;

use crate::models::{Commit, EnrichedCommit, PrInfo};

/// Sections produced by the classification ladder. Ordered by
/// [`Section::order_index`] when rendering.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Display,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Section {
    /// Breaking changes — supersedes the kind-based section.
    Breaking,
    Features,
    BugFixes,
    Performance,
    Refactor,
    Documentation,
    Tests,
    Build,
    Ci,
    Chore,
    /// Catch-all for commits that couldn't be confidently placed.
    Other,
}

impl Section {
    /// Parse a lenient section name from LLM output. Tolerates the common
    /// synonyms a model might produce (e.g. `bug-fixes`, `bug fixes`,
    /// `fix`, `fixes`). Returns `None` for unrecognised strings — the
    /// caller decides whether to fall back to [`Section::Other`].
    ///
    /// Centralised here so Tier 1, Tier 2, and Tier 3 share one parser.
    pub fn parse_lenient(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "breaking" | "breaking-changes" | "breaking changes" => Some(Self::Breaking),
            "features" | "feature" | "feat" => Some(Self::Features),
            "bug-fixes" | "bug_fixes" | "bug fixes" | "fix" | "fixes" => Some(Self::BugFixes),
            "performance" | "perf" => Some(Self::Performance),
            "refactor" | "refactoring" => Some(Self::Refactor),
            "documentation" | "docs" | "doc" => Some(Self::Documentation),
            "tests" | "test" => Some(Self::Tests),
            "build" => Some(Self::Build),
            "ci" => Some(Self::Ci),
            "chore" => Some(Self::Chore),
            "other" => Some(Self::Other),
            _ => None,
        }
    }

    /// Human-readable Markdown section header.
    pub fn header(self) -> &'static str {
        match self {
            Self::Breaking => "Breaking Changes",
            Self::Features => "Features",
            Self::BugFixes => "Bug Fixes",
            Self::Performance => "Performance",
            Self::Refactor => "Refactoring",
            Self::Documentation => "Documentation",
            Self::Tests => "Tests",
            Self::Build => "Build",
            Self::Ci => "CI",
            Self::Chore => "Chore",
            Self::Other => "Other",
        }
    }

    /// Stable ordering key — lower comes first in rendered output.
    pub fn order_index(self) -> u8 {
        match self {
            Self::Breaking => 0,
            Self::Features => 1,
            Self::BugFixes => 2,
            Self::Performance => 3,
            Self::Refactor => 4,
            Self::Documentation => 5,
            Self::Tests => 6,
            Self::Build => 7,
            Self::Ci => 8,
            Self::Chore => 9,
            Self::Other => 10,
        }
    }
}

/// Why a particular classification was chosen — recorded so audit logs
/// can explain "Tier 0 placed this commit here because…".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClassificationSource {
    /// Subject matched the Conventional Commits header.
    Conventional,
    /// Files-only heuristic — commit touched only paths matching a known
    /// pattern (e.g. only `Cargo.lock`, only `docs/`).
    FilesHeuristic {
        reason: String,
    },
    /// No signal matched; fell through to [`Section::Other`].
    Default,
    /// Reserved for future ladder tiers — included now so the model
    /// is forward-compatible without a migration.
    BatchedLlm,
    PerCommitLlm,
    Agentic,
}

/// One commit's classification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    pub section: Section,
    /// One-line summary used in rendered output. For Tier 0 this is the
    /// commit subject, optionally stripped of its Conventional prefix.
    pub summary: String,
    pub source: ClassificationSource,
    /// 0.0..=1.0 — Tier 0 deterministic placements are 1.0; later tiers
    /// produce lower confidences for commits that needed LLM judgment.
    pub confidence: f32,
}

/// A commit + its current classification + any PR enrichment.
///
/// `pr` is lifted from the [`EnrichedCommit`] this entry was built from
/// — it's *not* on [`Commit`] itself so that the git domain stays
/// unaware of PR-platform concepts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassifiedCommit {
    pub commit: Commit,
    /// PR data when enrichment ran and matched a PR for this commit.
    #[serde(default)]
    pub pr: Option<PrInfo>,
    pub classification: Classification,
}

impl ClassifiedCommit {
    /// Build from an [`EnrichedCommit`] and the classification placed
    /// on it by the ladder.
    pub fn from_enriched(enriched: EnrichedCommit, classification: Classification) -> Self {
        Self {
            commit: enriched.commit,
            pr: enriched.pr,
            classification,
        }
    }
}

/// All commits in a range, paired with classifications. Higher tiers
/// replace entries in-place rather than producing parallel lists.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Classified(pub Vec<ClassifiedCommit>);

impl Classified {
    pub fn new(items: Vec<ClassifiedCommit>) -> Self {
        Self(items)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ClassifiedCommit> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Group entries by their section, preserving the order they appeared
    /// in the input within each group.
    pub fn group_by_section(&self) -> Vec<(Section, Vec<&ClassifiedCommit>)> {
        use indexmap::IndexMap;
        let mut groups: IndexMap<Section, Vec<&ClassifiedCommit>> = IndexMap::new();
        for entry in &self.0 {
            groups
                .entry(entry.classification.section)
                .or_default()
                .push(entry);
        }
        let mut out: Vec<(Section, Vec<&ClassifiedCommit>)> = groups.into_iter().collect();
        out.sort_by_key(|(section, _)| section.order_index());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_ordering_is_breaking_first() {
        let mut sections = vec![
            Section::Other,
            Section::Features,
            Section::Breaking,
            Section::Chore,
        ];
        sections.sort_by_key(|s| s.order_index());
        assert_eq!(
            sections,
            vec![
                Section::Breaking,
                Section::Features,
                Section::Chore,
                Section::Other
            ]
        );
    }

    #[test]
    fn section_headers_are_title_case() {
        assert_eq!(Section::Breaking.header(), "Breaking Changes");
        assert_eq!(Section::BugFixes.header(), "Bug Fixes");
        assert_eq!(Section::Ci.header(), "CI");
    }

    #[test]
    fn parse_lenient_accepts_synonyms() {
        assert_eq!(Section::parse_lenient("bug-fixes"), Some(Section::BugFixes));
        assert_eq!(Section::parse_lenient("Bug Fixes"), Some(Section::BugFixes));
        assert_eq!(Section::parse_lenient("FIX"), Some(Section::BugFixes));
        assert_eq!(Section::parse_lenient("fixes"), Some(Section::BugFixes));
        assert_eq!(Section::parse_lenient("perf"), Some(Section::Performance));
        assert_eq!(Section::parse_lenient("docs"), Some(Section::Documentation));
        assert_eq!(Section::parse_lenient("breaking"), Some(Section::Breaking));
        assert_eq!(
            Section::parse_lenient("breaking-changes"),
            Some(Section::Breaking)
        );
        assert_eq!(Section::parse_lenient("???"), None);
        assert_eq!(
            Section::parse_lenient("  features  "),
            Some(Section::Features)
        );
    }
}
