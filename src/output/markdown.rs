//! Section-grouped Markdown renderer for [`Classified`] commits.
//!
//! This is the deterministic renderer used until the prose pass lands —
//! it produces real, if naive, release notes from Tier 0 alone. The
//! prose pass (Phase 8) will replace the formatting with LLM-generated
//! prose using the same input.

use crate::models::{Classified, ClassifiedCommit, ReleaseKind};

/// Options controlling the rendered output. Kept small for now; the
/// prose pass will subsume most styling decisions.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// Optional `## What's Changed`-style top-level header. `None` →
    /// only section subheaders are written.
    pub header: Option<String>,
    /// Optional trailing line such as `**Full Changelog**: <url>`.
    pub footer: Option<String>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            header: Some("What's Changed".to_string()),
            footer: None,
        }
    }
}

impl RenderOptions {
    /// Adjust defaults for a given release kind. Prereleases get a
    /// "Prerelease Notes — <label>" header; stable is unchanged.
    pub fn for_release(release_kind: &ReleaseKind) -> Self {
        match release_kind {
            ReleaseKind::Prerelease { label } => Self {
                header: Some(format!("Prerelease Notes — {label}")),
                footer: None,
            },
            _ => Self::default(),
        }
    }
}

/// Render a [`Classified`] release as GitHub-flavored Markdown.
pub fn render(classified: &Classified, opts: &RenderOptions) -> String {
    let mut out = String::new();

    if let Some(header) = &opts.header {
        out.push_str("## ");
        out.push_str(header);
        out.push_str("\n\n");
    }

    if classified.is_empty() {
        out.push_str("_No changes._\n");
        if let Some(footer) = &opts.footer {
            out.push('\n');
            out.push_str(footer);
            out.push('\n');
        }
        return out;
    }

    for (section, entries) in classified.group_by_section() {
        out.push_str("### ");
        out.push_str(section.header());
        out.push_str("\n\n");
        for entry in entries {
            out.push_str("- ");
            out.push_str(&format_bullet(entry));
            out.push('\n');
        }
        out.push('\n');
    }

    if let Some(footer) = &opts.footer {
        out.push_str(footer);
        out.push('\n');
    }

    out
}

/// Format a single classified commit as a bullet body (no leading dash).
///
/// The PR id is appended as `(#NN)` only if the summary doesn't already
/// contain it — squash-merge subjects already carry the PR suffix.
fn format_bullet(entry: &ClassifiedCommit) -> String {
    let summary = entry.classification.summary.trim();
    let needs_pr_suffix = match entry.commit.pr_id {
        Some(id) => !summary.contains(&format!("(#{id})")),
        None => false,
    };
    if needs_pr_suffix {
        format!("{summary} (#{})", entry.commit.pr_id.unwrap())
    } else {
        summary.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ladder::tier0;
    use crate::models::{Commit, Conventional, EnrichedCommit};

    fn commit(subject: &str, kind: Option<&str>, breaking: bool, pr_id: Option<u64>) -> Commit {
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
            author_email: "ada@x".to_string(),
            author_date: "2026-01-01T00:00:00+00:00".to_string(),
            parents: vec!["p".to_string()],
            subject: subject.to_string(),
            body: String::new(),
            files: vec!["x".to_string()],
            pr_id,
            conventional,
            breaking,
        }
    }

    fn enriched(
        subject: &str,
        kind: Option<&str>,
        breaking: bool,
        pr_id: Option<u64>,
    ) -> EnrichedCommit {
        EnrichedCommit::from_commit(commit(subject, kind, breaking, pr_id))
    }

    #[test]
    fn renders_grouped_sections_in_order() {
        let commits = vec![
            enriched("chore: bump", Some("chore"), false, None),
            enriched("feat: add login", Some("feat"), false, None),
            enriched("fix: race", Some("fix"), false, None),
            enriched("feat!: drop v1", Some("feat"), true, None),
        ];
        let classified = tier0::classify(&commits);
        let md = render(&classified, &RenderOptions::default());

        // Sections must appear Breaking → Features → BugFixes → Chore.
        let breaking = md.find("### Breaking Changes").unwrap();
        let features = md.find("### Features").unwrap();
        let fixes = md.find("### Bug Fixes").unwrap();
        let chore = md.find("### Chore").unwrap();
        assert!(breaking < features);
        assert!(features < fixes);
        assert!(fixes < chore);

        assert!(md.contains("- drop v1"));
        assert!(md.contains("- add login"));
        assert!(md.contains("- race"));
    }

    #[test]
    fn empty_classified_renders_no_changes_line() {
        let md = render(&Classified::default(), &RenderOptions::default());
        assert!(md.contains("_No changes._"));
    }

    #[test]
    fn header_can_be_disabled() {
        let commits = vec![enriched("feat: x", Some("feat"), false, None)];
        let classified = tier0::classify(&commits);
        let md = render(
            &classified,
            &RenderOptions {
                header: None,
                footer: None,
            },
        );
        // No `## ` (level-2) header line. `### Features` is fine.
        assert!(
            !md.lines().any(|l| l.starts_with("## ")),
            "no top-level `## ` header expected: {md}"
        );
        assert!(md.contains("### Features"));
    }

    #[test]
    fn footer_appears_at_end() {
        let commits = vec![enriched("feat: x", Some("feat"), false, None)];
        let classified = tier0::classify(&commits);
        let md = render(
            &classified,
            &RenderOptions {
                header: None,
                footer: Some("**Full Changelog**: https://example.com/compare".to_string()),
            },
        );
        assert!(
            md.trim_end()
                .ends_with("**Full Changelog**: https://example.com/compare")
        );
    }

    #[test]
    fn appends_pr_suffix_when_summary_lacks_it() {
        let mut c = commit("Add login", None, false, Some(42));
        // Force into Features by giving it a conventional parse.
        c.conventional = Some(Conventional {
            kind: "feat".into(),
            scope: None,
            breaking: false,
            description: "Add login".into(),
        });
        let classified = tier0::classify(&[EnrichedCommit::from_commit(c)]);
        let md = render(&classified, &RenderOptions::default());
        assert!(md.contains("- Add login (#42)"));
    }

    #[test]
    fn does_not_double_append_pr_suffix() {
        let c = commit("Add login (#42)", None, false, Some(42));
        let classified = tier0::classify(&[EnrichedCommit::from_commit(c)]);
        let md = render(&classified, &RenderOptions::default());
        // The Tier 0 default classification keeps the subject including
        // the existing `(#42)` suffix; renderer must not re-append.
        assert_eq!(md.matches("(#42)").count(), 1);
    }
}
