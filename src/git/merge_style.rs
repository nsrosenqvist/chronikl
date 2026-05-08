//! Merge-style detection for a commit range.
//!
//! Inspects commit subjects and parent counts to guess whether the
//! branch was integrated via squash-merge, rebase, or merge commits.
//! The result drives how aggressively the classification ladder runs:
//! squash repos have curated subjects (PR titles), rebase repos have
//! raw messages that often need diff-aware classification.

use std::sync::OnceLock;

use regex::Regex;

use crate::models::{Commit, MergeStyle};

/// Subjects ending in ` (#NNN)` are typical of GitHub squash-merge.
fn squash_suffix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\s\(#\d+\)\s*$").expect("static regex compiles"))
}

/// Default merge-commit subjects from `git merge` / GitHub merge-commits.
fn merge_subject_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?:Merge pull request #\d+|Merge branch |Merge remote-tracking )")
            .expect("static regex compiles")
    })
}

/// Decide a [`MergeStyle`] from a commit list.
///
/// Heuristic thresholds:
/// - `squash` if >50% of commits look like `... (#NNN)` PR-title squashes.
/// - `merge`  if >20% of commits are `Merge pull request #...` /
///   `Merge branch ...` subjects (with parent_count >= 2).
/// - `rebase` if neither pattern is meaningfully present.
/// - `mixed`  otherwise.
///
/// Empty input returns [`MergeStyle::Rebase`] — the safe default that
/// runs the full ladder.
pub fn detect(commits: &[Commit]) -> MergeStyle {
    if commits.is_empty() {
        return MergeStyle::Rebase;
    }

    let total = commits.len() as f32;
    let squash_count = commits
        .iter()
        .filter(|c| squash_suffix_re().is_match(&c.subject))
        .count() as f32;
    let merge_count = commits
        .iter()
        .filter(|c| c.is_merge() || merge_subject_re().is_match(&c.subject))
        .count() as f32;

    let squash_ratio = squash_count / total;
    let merge_ratio = merge_count / total;

    if squash_ratio > 0.5 {
        MergeStyle::Squash
    } else if merge_ratio > 0.2 {
        MergeStyle::Merge
    } else if squash_ratio < 0.1 && merge_ratio < 0.05 {
        MergeStyle::Rebase
    } else {
        MergeStyle::Mixed
    }
}

/// Extract a PR ID from a squash-merge subject like `... (#123)`.
pub fn extract_pr_id(subject: &str) -> Option<u64> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\(#(\d+)\)\s*$").expect("static regex compiles"));
    re.captures(subject)?.get(1)?.as_str().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(subject: &str, parents: usize) -> Commit {
        Commit {
            sha: "0".repeat(40),
            short_sha: "0000000".to_string(),
            author_name: "x".to_string(),
            author_email: "x@example.com".to_string(),
            author_date: "2026-01-01T00:00:00+00:00".to_string(),
            parents: vec!["p".to_string(); parents],
            subject: subject.to_string(),
            body: String::new(),
            files: vec![],
            pr_id: None,
            conventional: None,
            breaking: false,
        }
    }

    #[test]
    fn empty_is_rebase() {
        assert_eq!(detect(&[]), MergeStyle::Rebase);
    }

    #[test]
    fn squash_dominated() {
        let commits = vec![
            commit("Add login flow (#10)", 1),
            commit("Fix race in cache (#11)", 1),
            commit("Bump deps (#12)", 1),
            commit("Refactor handler (#13)", 1),
        ];
        assert_eq!(detect(&commits), MergeStyle::Squash);
    }

    #[test]
    fn merge_dominated() {
        let commits = vec![
            commit("Merge pull request #1 from foo/bar", 2),
            commit("Add foo", 1),
            commit("Merge pull request #2 from baz/qux", 2),
            commit("Fix baz", 1),
        ];
        assert_eq!(detect(&commits), MergeStyle::Merge);
    }

    #[test]
    fn rebase_dominated() {
        let commits = vec![
            commit("Add login flow", 1),
            commit("Fix race in cache", 1),
            commit("Bump deps", 1),
            commit("wip", 1),
        ];
        assert_eq!(detect(&commits), MergeStyle::Rebase);
    }

    #[test]
    fn mixed_signals() {
        let commits = vec![
            commit("Add login (#1)", 1),
            commit("raw fix", 1),
            commit("another raw", 1),
            commit("more raw", 1),
        ];
        // 25% squash, 0% merge → mixed
        assert_eq!(detect(&commits), MergeStyle::Mixed);
    }

    #[test]
    fn extract_pr_id_basic() {
        assert_eq!(extract_pr_id("Add login (#42)"), Some(42));
        assert_eq!(extract_pr_id("Add login (#42) "), Some(42));
        assert_eq!(extract_pr_id("Add login"), None);
        assert_eq!(extract_pr_id("(#) empty"), None);
    }
}
