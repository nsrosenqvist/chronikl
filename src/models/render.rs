//! Rendering settings shared by the prose pass and the output shaper.
//!
//! [`RenderSettings`] is the *resolved* view: every layer (CLI flags,
//! env, TOML, the voice's frontmatter, built-in defaults) has already
//! been folded in by [`crate::config::Config::render_settings`]. The
//! prose prompts read it to ask the model for the right shape; the
//! output shaper reads it to enforce that shape on whatever came back.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use crate::constants::{
    DEFAULT_FALLBACK_FIXES, DEFAULT_FALLBACK_OTHER, DEFAULT_FALLBACK_PERFORMANCE,
};
use crate::models::{Classified, Section};

/// Text format of the final output.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Display, EnumString,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum TextFormat {
    /// GitHub-flavored Markdown.
    #[default]
    Markdown,
    /// Plain text: `- ` bullets, no emphasis, code spans, links or `#`
    /// headings. For stores and UIs that show text verbatim.
    Plain,
}

/// What kind of work a range with nothing user-facing held, which picks
/// its fallback line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum FallbackKind {
    /// At least one commit is a bug fix.
    Fixes,
    /// No fixes, but at least one performance commit.
    Performance,
    /// Anything else (docs, tests, CI, refactors, internal features, an
    /// empty range).
    Other,
}

impl FallbackKind {
    /// Pick the kind from a range's classified sections: fixes win over
    /// performance, which wins over everything else.
    pub fn of(classified: &Classified) -> Self {
        let has = |s: Section| classified.iter().any(|c| c.classification.section == s);
        if has(Section::BugFixes) {
            Self::Fixes
        } else if has(Section::Performance) {
            Self::Performance
        } else {
            Self::Other
        }
    }
}

/// Custom "nothing user-facing" lines. A per-kind line wins over
/// `line`, which wins over the built-in default for the kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FallbackLines {
    pub line: Option<String>,
    pub fixes: Option<String>,
    pub performance: Option<String>,
    pub other: Option<String>,
}

impl FallbackLines {
    /// The line for `kind`.
    pub fn for_kind(&self, kind: FallbackKind) -> &str {
        let (specific, default) = match kind {
            FallbackKind::Fixes => (&self.fixes, DEFAULT_FALLBACK_FIXES),
            FallbackKind::Performance => (&self.performance, DEFAULT_FALLBACK_PERFORMANCE),
            FallbackKind::Other => (&self.other, DEFAULT_FALLBACK_OTHER),
        };
        specific
            .as_deref()
            .or(self.line.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(default)
    }

    /// Whether any line is set.
    pub fn any_set(&self) -> bool {
        self.line.is_some()
            || self.fixes.is_some()
            || self.performance.is_some()
            || self.other.is_some()
    }
}

/// Fully resolved rendering settings for one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderSettings {
    /// `false` renders one flat bullet list with no section headings.
    pub headings: bool,
    /// When `Some`, only commits in these sections are kept.
    pub include_sections: Option<Vec<Section>>,
    /// Commits in these sections are dropped before the prose pass.
    pub exclude_sections: Vec<Section>,
    /// Keep at most this many bullets (whole bullets, in output order).
    pub max_bullets: Option<usize>,
    /// Keep the whole output within this many characters, cutting at
    /// whole bullets.
    pub max_chars: Option<usize>,
    /// `false` drops the "Full Changelog" compare link and any other
    /// trailer.
    pub footer: bool,
    pub format: TextFormat,
    /// The notes are for the product's users, not its developers. The
    /// prose model is told to keep only changes a user would notice, and
    /// when nothing in the range is user-facing (every commit filtered
    /// out, or the model finds nothing a user would notice) or the
    /// model can't be reached, the output is one friendly line picked
    /// by [`FallbackKind`] instead of an empty or technical text.
    pub user_facing: bool,
    /// Custom lines for that case.
    pub fallback_lines: FallbackLines,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            headings: true,
            include_sections: None,
            exclude_sections: Vec::new(),
            max_bullets: None,
            max_chars: None,
            footer: true,
            format: TextFormat::Markdown,
            user_facing: false,
            fallback_lines: FallbackLines::default(),
        }
    }
}

impl RenderSettings {
    /// Whether commits classified into `section` reach the prose pass
    /// and the output.
    pub fn keeps(&self, section: Section) -> bool {
        let included = match &self.include_sections {
            Some(list) => list.contains(&section),
            None => true,
        };
        included && !self.exclude_sections.contains(&section)
    }

    /// The subset of `classified` this run renders, in the same order.
    pub fn filter(&self, classified: &Classified) -> Classified {
        Classified::new(
            classified
                .iter()
                .filter(|c| self.keeps(c.classification.section))
                .cloned()
                .collect(),
        )
    }

    /// The "nothing user-facing" line for a range, picked from its
    /// classified sections (all commits, filtered or not).
    pub fn fallback_line(&self, classified: &Classified) -> &str {
        self.fallback_lines.for_kind(FallbackKind::of(classified))
    }

    /// Whether any setting asks for a document shape other than
    /// chronikl's historical default (sectioned Markdown, no limits).
    pub fn changes_shape(&self) -> bool {
        !self.headings
            || self.user_facing
            || self.max_bullets.is_some()
            || self.max_chars.is_some()
            || self.format != TextFormat::Markdown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Classification, ClassificationSource, ClassifiedCommit, Commit};

    fn entry(section: Section) -> ClassifiedCommit {
        ClassifiedCommit {
            commit: Commit {
                sha: "0".repeat(40),
                short_sha: "0000000".into(),
                author_name: "a".into(),
                author_email: "a@x".into(),
                author_date: "2026-01-01T00:00:00+00:00".into(),
                parents: vec![],
                subject: section.to_string(),
                body: String::new(),
                files: vec![],
                pr_id: None,
                conventional: None,
                breaking: false,
            },
            pr: None,
            classification: Classification {
                section,
                summary: section.to_string(),
                source: ClassificationSource::Conventional,
                confidence: 1.0,
            },
        }
    }

    #[test]
    fn default_keeps_every_section() {
        let s = RenderSettings::default();
        for section in [Section::Features, Section::Tests, Section::Other] {
            assert!(s.keeps(section));
        }
        assert!(!s.changes_shape());
    }

    #[test]
    fn exclude_drops_listed_sections() {
        let s = RenderSettings {
            exclude_sections: vec![Section::Tests, Section::Documentation],
            ..Default::default()
        };
        let classified = Classified::new(vec![
            entry(Section::Features),
            entry(Section::Tests),
            entry(Section::Documentation),
            entry(Section::BugFixes),
        ]);
        let kept: Vec<Section> = s
            .filter(&classified)
            .iter()
            .map(|c| c.classification.section)
            .collect();
        assert_eq!(kept, vec![Section::Features, Section::BugFixes]);
    }

    #[test]
    fn include_keeps_only_listed_sections_and_exclude_still_applies() {
        let s = RenderSettings {
            include_sections: Some(vec![Section::Features, Section::BugFixes]),
            exclude_sections: vec![Section::BugFixes],
            ..Default::default()
        };
        assert!(s.keeps(Section::Features));
        assert!(!s.keeps(Section::BugFixes));
        assert!(!s.keeps(Section::Performance));
    }

    fn range(sections: &[Section]) -> Classified {
        Classified::new(sections.iter().map(|&s| entry(s)).collect())
    }

    #[test]
    fn fallback_kind_follows_the_sections() {
        // All-internal fixes-only range.
        assert_eq!(
            FallbackKind::of(&range(&[Section::BugFixes, Section::Ci])),
            FallbackKind::Fixes
        );
        // Performance-only range.
        assert_eq!(
            FallbackKind::of(&range(&[Section::Performance])),
            FallbackKind::Performance
        );
        // Fixes win over performance.
        assert_eq!(
            FallbackKind::of(&range(&[Section::Performance, Section::BugFixes])),
            FallbackKind::Fixes
        );
        // Docs/tests-only range.
        assert_eq!(
            FallbackKind::of(&range(&[Section::Documentation, Section::Tests])),
            FallbackKind::Other
        );
        // Empty range.
        assert_eq!(
            FallbackKind::of(&Classified::default()),
            FallbackKind::Other
        );
    }

    #[test]
    fn fallback_lines_default_per_kind() {
        let s = RenderSettings::default();
        assert_eq!(
            s.fallback_line(&range(&[Section::BugFixes])),
            DEFAULT_FALLBACK_FIXES
        );
        assert_eq!(
            s.fallback_line(&range(&[Section::Performance])),
            DEFAULT_FALLBACK_PERFORMANCE
        );
        assert_eq!(
            s.fallback_line(&range(&[Section::Tests])),
            DEFAULT_FALLBACK_OTHER
        );
    }

    #[test]
    fn custom_fallback_lines_override_defaults() {
        let lines = FallbackLines {
            line: Some("Updates.".into()),
            fixes: Some("Fixes.".into()),
            performance: None,
            other: Some("  ".into()),
        };
        assert_eq!(lines.for_kind(FallbackKind::Fixes), "Fixes.");
        assert_eq!(lines.for_kind(FallbackKind::Performance), "Updates.");
        // A blank per-kind line falls through to the built-in default.
        assert_eq!(lines.for_kind(FallbackKind::Other), DEFAULT_FALLBACK_OTHER);
        assert!(lines.any_set());
        assert!(!FallbackLines::default().any_set());
    }
}
