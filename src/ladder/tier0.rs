//! Tier 0 — deterministic, no-LLM classification.
//!
//! For each commit, attempt a confident placement from cheap signals:
//!
//! 1. Conventional Commits prefix → kind-derived section, breaking flag
//!    promotes to [`Section::Breaking`].
//! 2. Files-only heuristics for non-Conventional subjects (only
//!    `Cargo.lock`, only `docs/`, only `.github/workflows/`, etc.).
//! 3. Otherwise → [`Section::Other`] with low confidence so later tiers
//!    can pick it up.
//!
//! All placements at this tier carry confidence = 1.0 (deterministic) or
//! 0.5 (Default fallback) so the Tier 1→2 escalator can decide what to
//! reclassify.

use std::path::Path;

use crate::models::{
    Classification, ClassificationSource, Classified, ClassifiedCommit, Commit, EnrichedCommit,
    Section,
};

/// Files that, when they're the *only* paths touched, justify a chore-deps
/// classification without any LLM input.
const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "go.sum",
    "Pipfile.lock",
    "poetry.lock",
    "uv.lock",
    "composer.lock",
    "Gemfile.lock",
];

/// Run Tier 0 classification over the given enriched commits,
/// returning a [`Classified`] in the same order. Each output entry's
/// `pr` field is lifted from its input [`EnrichedCommit`].
pub fn classify(commits: &[EnrichedCommit]) -> Classified {
    Classified::new(commits.iter().map(classify_one).collect())
}

fn classify_one(entry: &EnrichedCommit) -> ClassifiedCommit {
    let classification = if let Some(c) = classify_conventional(&entry.commit) {
        c
    } else if let Some(c) = classify_files_only(&entry.commit) {
        c
    } else {
        Classification {
            section: Section::Other,
            summary: entry.commit.subject.clone(),
            source: ClassificationSource::Default,
            // Low confidence — the ladder uses this to decide whether to
            // escalate to Tier 1/2 LLM passes.
            confidence: 0.5,
        }
    };
    ClassifiedCommit {
        commit: entry.commit.clone(),
        pr: entry.pr.clone(),
        classification,
    }
}

/// Classify via Conventional Commits parse (if any).
fn classify_conventional(commit: &Commit) -> Option<Classification> {
    let conv = commit.conventional.as_ref()?;
    let section = if commit.breaking || conv.breaking {
        Section::Breaking
    } else {
        section_from_kind(&conv.kind)
    };
    Some(Classification {
        section,
        // Drop the redundant `feat:`/`fix:` prefix — the section header
        // already conveys the kind. Keep the description as-is.
        summary: conv.description.clone(),
        source: ClassificationSource::Conventional,
        confidence: 1.0,
    })
}

fn section_from_kind(kind: &str) -> Section {
    match kind {
        "feat" => Section::Features,
        "fix" => Section::BugFixes,
        "perf" => Section::Performance,
        "refactor" => Section::Refactor,
        "docs" => Section::Documentation,
        "test" | "tests" => Section::Tests,
        "build" => Section::Build,
        "ci" => Section::Ci,
        "chore" | "style" => Section::Chore,
        // Conventional doesn't define `revert` formally, but it's common.
        "revert" => Section::Other,
        _ => Section::Other,
    }
}

/// Classify based on which files the commit touched, when no Conventional
/// header is present. Only fires when *every* file matches a single
/// pattern — mixed commits fall through.
fn classify_files_only(commit: &Commit) -> Option<Classification> {
    if commit.files.is_empty() {
        return None;
    }

    // Lockfiles only.
    if commit.files.iter().all(|f| {
        let name = Path::new(f)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        LOCKFILES.contains(&name)
    }) {
        return Some(deterministic(
            Section::Chore,
            &commit.subject,
            "files-only-lockfiles",
        ));
    }

    // docs/ only.
    if commit.files.iter().all(|f| f.starts_with("docs/")) {
        return Some(deterministic(
            Section::Documentation,
            &commit.subject,
            "files-only-docs-dir",
        ));
    }

    // .github/workflows/ only.
    if commit
        .files
        .iter()
        .all(|f| f.starts_with(".github/workflows/"))
    {
        return Some(deterministic(
            Section::Ci,
            &commit.subject,
            "files-only-workflows",
        ));
    }

    // Top-level `*.md` only.
    if commit
        .files
        .iter()
        .all(|f| !f.contains('/') && f.ends_with(".md"))
    {
        return Some(deterministic(
            Section::Documentation,
            &commit.subject,
            "files-only-root-markdown",
        ));
    }

    None
}

fn deterministic(section: Section, summary: &str, reason: &'static str) -> Classification {
    Classification {
        section,
        summary: summary.to_string(),
        source: ClassificationSource::FilesHeuristic {
            reason: reason.to_string(),
        },
        confidence: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Conventional;

    fn commit(subject: &str, kind: Option<&str>, breaking: bool, files: &[&str]) -> Commit {
        let conventional = kind.map(|k| Conventional {
            kind: k.to_string(),
            scope: None,
            breaking,
            description: subject.split(": ").nth(1).unwrap_or(subject).to_string(),
        });
        Commit {
            sha: "0".repeat(40),
            short_sha: "0000000".to_string(),
            author_name: "ada".to_string(),
            author_email: "ada@example.com".to_string(),
            author_date: "2026-01-01T00:00:00+00:00".to_string(),
            parents: vec!["p".to_string()],
            subject: subject.to_string(),
            body: String::new(),
            files: files.iter().map(|s| s.to_string()).collect(),
            pr_id: None,
            conventional,
            breaking,
        }
    }

    fn enriched(
        subject: &str,
        kind: Option<&str>,
        breaking: bool,
        files: &[&str],
    ) -> EnrichedCommit {
        EnrichedCommit::from_commit(commit(subject, kind, breaking, files))
    }

    #[test]
    fn feat_goes_to_features() {
        let c = classify_one(&enriched(
            "feat: add login",
            Some("feat"),
            false,
            &["src/lib.rs"],
        ));
        assert_eq!(c.classification.section, Section::Features);
        assert_eq!(c.classification.summary, "add login");
        assert_eq!(c.classification.confidence, 1.0);
    }

    #[test]
    fn fix_goes_to_bug_fixes() {
        let c = classify_one(&enriched("fix: race", Some("fix"), false, &["src/lib.rs"]));
        assert_eq!(c.classification.section, Section::BugFixes);
    }

    #[test]
    fn breaking_supersedes_kind() {
        let c = classify_one(&enriched(
            "feat!: drop legacy API",
            Some("feat"),
            true,
            &["src/lib.rs"],
        ));
        assert_eq!(c.classification.section, Section::Breaking);
    }

    #[test]
    fn lockfile_only_is_chore() {
        let c = classify_one(&enriched("Bump deps", None, false, &["Cargo.lock"]));
        assert_eq!(c.classification.section, Section::Chore);
        assert!(matches!(
            c.classification.source,
            ClassificationSource::FilesHeuristic { .. }
        ));
    }

    #[test]
    fn multiple_lockfiles_only_is_chore() {
        let c = classify_one(&enriched(
            "Bump deps",
            None,
            false,
            &["Cargo.lock", "package-lock.json"],
        ));
        assert_eq!(c.classification.section, Section::Chore);
    }

    #[test]
    fn lockfile_plus_other_file_is_not_chore() {
        let c = classify_one(&enriched(
            "Bump deps and tweak",
            None,
            false,
            &["Cargo.lock", "src/main.rs"],
        ));
        assert_eq!(c.classification.section, Section::Other);
        assert_eq!(c.classification.confidence, 0.5);
    }

    #[test]
    fn docs_dir_only_is_documentation() {
        let c = classify_one(&enriched("Update guide", None, false, &["docs/guide.md"]));
        assert_eq!(c.classification.section, Section::Documentation);
    }

    #[test]
    fn workflows_only_is_ci() {
        let c = classify_one(&enriched(
            "Bump action",
            None,
            false,
            &[".github/workflows/ci.yml"],
        ));
        assert_eq!(c.classification.section, Section::Ci);
    }

    #[test]
    fn root_markdown_only_is_documentation() {
        let c = classify_one(&enriched("README tweaks", None, false, &["README.md"]));
        assert_eq!(c.classification.section, Section::Documentation);
    }

    #[test]
    fn random_subject_with_random_files_falls_through() {
        let c = classify_one(&enriched("wip", None, false, &["src/main.rs"]));
        assert_eq!(c.classification.section, Section::Other);
        assert!(matches!(
            c.classification.source,
            ClassificationSource::Default
        ));
        assert_eq!(c.classification.confidence, 0.5);
    }

    #[test]
    fn classify_returns_input_order() {
        let commits = vec![
            enriched("feat: a", Some("feat"), false, &["x"]),
            enriched("fix: b", Some("fix"), false, &["y"]),
            enriched("chore: c", Some("chore"), false, &["z"]),
        ];
        let classified = classify(&commits);
        let summaries: Vec<&str> = classified
            .iter()
            .map(|c| c.classification.summary.as_str())
            .collect();
        assert_eq!(summaries, vec!["a", "b", "c"]);
    }

    #[test]
    fn group_by_section_orders_breaking_first() {
        let commits = vec![
            enriched("chore: c", Some("chore"), false, &["x"]),
            enriched("feat!: bx", Some("feat"), true, &["y"]),
            enriched("feat: f", Some("feat"), false, &["z"]),
        ];
        let classified = classify(&commits);
        let groups = classified.group_by_section();
        let sections: Vec<Section> = groups.iter().map(|(s, _)| *s).collect();
        assert_eq!(
            sections,
            vec![Section::Breaking, Section::Features, Section::Chore]
        );
    }
}
