//! Prompt assembly for the prose pass.
//!
//! Builds the system + user prompts the prose model sees. The system
//! prompt is the loaded voice plus any addenda; the user prompt is a
//! structured listing of the classified commits the model should turn
//! into prose.

use crate::models::{
    Classified, ClassifiedCommit, MergeStyle, ReleaseKind, VersionBump, VersionScheme,
};
use crate::voice::Voice;

/// Build the system prompt for the prose pass: the voice's body, plus
/// any extra instructions appended after a blank line.
///
/// `extra_instructions` typically comes from `voice.extra_instructions`
/// in the TOML config. `inline_prompt` is the `--prompt "<TEXT>"` CLI
/// flag — applied last so a one-shot override always wins.
pub fn build_system_prompt(
    voice: &Voice,
    extra_instructions: Option<&str>,
    inline_prompt: Option<&str>,
    release_kind: &ReleaseKind,
    bump: VersionBump,
    scheme: Option<VersionScheme>,
) -> String {
    let mut out = voice.system_prompt.clone();

    // Auto-addenda are appended *between* the voice and the
    // user-supplied instructions so the user's `--prompt` always wins
    // when there's a conflict (e.g. "ignore the framing, this is
    // internal"). Order: voice → bump → prerelease → user extras.
    if let Some(addendum) = crate::prompts::bump_addendum(bump, scheme) {
        out.push_str("\n\n");
        out.push_str(addendum);
    }
    if let ReleaseKind::Prerelease { label } = release_kind {
        out.push_str("\n\n");
        out.push_str(&crate::prompts::prerelease_addendum(label));
    }

    for piece in [extra_instructions, inline_prompt].into_iter().flatten() {
        let trimmed = piece.trim();
        if !trimmed.is_empty() {
            out.push_str("\n\n");
            out.push_str(trimmed);
        }
    }
    out
}

/// Hard cap on commit-body characters under `rich_context`. Matches
/// `tier1::BODY_PREVIEW_CHARS` so the prose pass and the classification
/// ladder see comparable amounts of body text.
const COMMIT_BODY_PREVIEW: usize = 600;

/// Hard cap on PR-body characters under `rich_context`. Matches the
/// `prompts::PR_BODY_PREVIEW` used by the classification ladder.
const PR_BODY_PREVIEW: usize = 1000;

/// Build the user prompt: a structured listing of the classified
/// commits the model should turn into prose.
///
/// Format:
///
/// ```text
/// Generate release notes for the range <from>..<to> (<N> commits).
///
/// Detected merge style: <style>.
///
/// Commits, grouped by section:
///
/// ## Breaking Changes
/// - Drop support for Node 18  [conv, conf=1.0]
///   PR #42: Remove legacy runtime  labels: breaking-change
///
/// ## Features
/// - Add login flow  [llm-batch, conf=0.8]
/// ```
#[allow(clippy::too_many_arguments)]
pub fn build_user_prompt(
    classified: &Classified,
    from_ref: Option<&str>,
    to_ref: &str,
    merge_style: MergeStyle,
    release_kind: &ReleaseKind,
    bump: VersionBump,
    scheme: Option<VersionScheme>,
    rich_context: bool,
) -> String {
    let mut out = String::new();
    let count = classified.len();
    let range_str = match from_ref {
        Some(from) => format!("{from}..{to_ref}"),
        None => to_ref.to_string(),
    };
    out.push_str(&format!(
        "Generate release notes for the range `{range_str}` ({count} commits).\n\n"
    ));
    out.push_str(&format!("Detected merge style: {merge_style}.\n"));
    out.push_str(&format!("Version bump: {}.\n", bump.as_str()));
    if let Some(s) = scheme {
        out.push_str(&format!("Version scheme: {}.\n", s.as_str()));
    }
    match release_kind {
        ReleaseKind::Prerelease { label } => {
            out.push_str(&format!("Release kind: prerelease ({label}).\n\n"));
        }
        ReleaseKind::Stable => {
            out.push_str("Release kind: stable.\n\n");
        }
        ReleaseKind::Untagged => {
            out.push_str("Release kind: preview (no release tag at the upper bound).\n\n");
        }
    }

    if classified.is_empty() {
        out.push_str("No commits in range.\n");
        return out;
    }

    out.push_str("Commits, grouped by section:\n\n");
    for (section, entries) in classified.group_by_section() {
        out.push_str(&format!("## {}\n", section.header()));
        for entry in entries {
            push_entry(&mut out, entry, rich_context);
        }
        out.push('\n');
    }

    out.push_str(
        "Write release notes in your configured voice. Output GitHub-flavored \
         Markdown only. Use the section names above as `### Section Name` \
         headers and skip empty sections. Do not invent entries that aren't \
         in the list above. Do not include a top-level title or a \
         `## What's Changed` wrapper.\n",
    );
    out
}

fn push_entry(out: &mut String, entry: &ClassifiedCommit, rich_context: bool) {
    let summary = entry.classification.summary.trim();
    let source = source_tag(&entry.classification.source);
    let conf = entry.classification.confidence;
    out.push_str(&format!("- {summary}  [{source}, conf={conf:.1}]"));
    if let Some(pr_id) = entry.commit.pr_id {
        out.push_str(&format!("  (#{pr_id})"));
    }
    out.push('\n');

    if let Some(pr) = &entry.pr {
        out.push_str(&format!("  PR #{}: {}", pr.number, pr.title));
        if !pr.labels.is_empty() {
            out.push_str(&format!("  labels: {}", pr.labels.join(", ")));
        }
        out.push('\n');
    }

    if rich_context {
        push_indented_block(
            out,
            "commit-body",
            &truncate(&entry.commit.body, COMMIT_BODY_PREVIEW),
        );
        if let Some(pr) = &entry.pr {
            push_indented_block(out, "pr-body", &truncate(&pr.body, PR_BODY_PREVIEW));
        }
    }
}

/// Write a labeled, doubly-indented multi-line block under the current
/// bullet. Skips silently when the body is empty after trimming so we
/// don't emit dangling labels.
fn push_indented_block(out: &mut String, label: &str, body: &str) {
    let body = body.trim();
    if body.is_empty() {
        return;
    }
    out.push_str(&format!("  {label}:\n"));
    for line in body.lines() {
        out.push_str("    ");
        out.push_str(line);
        out.push('\n');
    }
}

/// Truncate to at most `max_chars` characters (not bytes), appending an
/// ellipsis when content was cut. Counts by `chars()` so we don't slice
/// a multi-byte UTF-8 codepoint.
fn truncate(s: &str, max_chars: usize) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let prefix: String = trimmed.chars().take(max_chars).collect();
    format!("{prefix}…")
}

fn source_tag(source: &crate::models::ClassificationSource) -> &'static str {
    use crate::models::ClassificationSource::*;
    match source {
        Conventional => "conv",
        FilesHeuristic { .. } => "files",
        Default => "default",
        BatchedLlm => "llm-batch",
        PerCommitLlm => "llm-per-commit",
        Agentic => "llm-agent",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        Classification, ClassificationSource, Classified, ClassifiedCommit, Commit, PrInfo, Section,
    };
    use crate::voice::Voice;

    fn voice(body: &str) -> Voice {
        Voice {
            system_prompt: body.to_string(),
            is_custom: false,
            bundled_name: None,
        }
    }

    fn entry(subject: &str, section: Section, source: ClassificationSource) -> ClassifiedCommit {
        ClassifiedCommit {
            commit: Commit {
                sha: "0".repeat(40),
                short_sha: "0000000".into(),
                author_name: "ada".into(),
                author_email: "ada@x".into(),
                author_date: "2026-01-01T00:00:00+00:00".into(),
                parents: vec!["p".into()],
                subject: subject.into(),
                body: String::new(),
                files: vec![],
                pr_id: None,
                conventional: None,
                breaking: false,
            },
            pr: None,
            classification: Classification {
                section,
                summary: subject.into(),
                source,
                confidence: 1.0,
            },
        }
    }

    #[test]
    fn build_system_prompt_uses_voice_body() {
        let v = voice("be terse");
        assert_eq!(
            build_system_prompt(
                &v,
                None,
                None,
                &ReleaseKind::Stable,
                VersionBump::Unknown,
                None
            ),
            "be terse"
        );
    }

    #[test]
    fn build_system_prompt_appends_extra_then_inline() {
        let v = voice("be terse");
        let out = build_system_prompt(
            &v,
            Some("mention release manager"),
            Some("cite PRs"),
            &ReleaseKind::Stable,
            VersionBump::Unknown,
            None,
        );
        assert!(out.starts_with("be terse"));
        assert!(out.contains("mention release manager"));
        assert!(out.contains("cite PRs"));
        let i_extra = out.find("mention release manager").unwrap();
        let i_inline = out.find("cite PRs").unwrap();
        assert!(i_extra < i_inline);
    }

    #[test]
    fn build_system_prompt_skips_empty_pieces() {
        let v = voice("be terse");
        let out = build_system_prompt(
            &v,
            Some("   "),
            None,
            &ReleaseKind::Stable,
            VersionBump::Unknown,
            None,
        );
        assert_eq!(out, "be terse");
    }

    #[test]
    fn build_system_prompt_adds_prerelease_addendum() {
        let v = voice("be terse");
        let out = build_system_prompt(
            &v,
            None,
            None,
            &ReleaseKind::Prerelease {
                label: "rc.1".into(),
            },
            VersionBump::Unknown,
            None,
        );
        assert!(out.starts_with("be terse"));
        assert!(out.contains("prerelease (rc.1)"));
        assert!(out.contains("experimental"));
    }

    #[test]
    fn prerelease_addendum_after_voice_before_user_extras() {
        let v = voice("voice body");
        let out = build_system_prompt(
            &v,
            Some("user extra"),
            Some("inline"),
            &ReleaseKind::Prerelease {
                label: "beta.1".into(),
            },
            VersionBump::Unknown,
            None,
        );
        let i_voice = out.find("voice body").unwrap();
        let i_pre = out.find("prerelease").unwrap();
        let i_extra = out.find("user extra").unwrap();
        let i_inline = out.find("inline").unwrap();
        assert!(i_voice < i_pre);
        assert!(i_pre < i_extra);
        assert!(i_extra < i_inline);
    }

    #[test]
    fn bump_addendum_added_for_each_kind() {
        let v = voice("v");
        let major = build_system_prompt(
            &v,
            None,
            None,
            &ReleaseKind::Stable,
            VersionBump::Major,
            None,
        );
        assert!(major.contains("major release"));
        assert!(major.contains("breaking changes"));

        let minor = build_system_prompt(
            &v,
            None,
            None,
            &ReleaseKind::Stable,
            VersionBump::Minor,
            None,
        );
        assert!(minor.contains("minor release"));
        assert!(minor.contains("new features"));

        let patch = build_system_prompt(
            &v,
            None,
            None,
            &ReleaseKind::Stable,
            VersionBump::Patch,
            None,
        );
        assert!(patch.contains("patch release"));

        let initial = build_system_prompt(
            &v,
            None,
            None,
            &ReleaseKind::Stable,
            VersionBump::Initial,
            None,
        );
        assert!(initial.contains("initial public release"));

        let unknown = build_system_prompt(
            &v,
            None,
            None,
            &ReleaseKind::Stable,
            VersionBump::Unknown,
            None,
        );
        assert_eq!(unknown, "v"); // no bump addendum
    }

    #[test]
    fn bump_and_prerelease_compose() {
        let v = voice("v");
        // Major prerelease — both addenda should fire.
        let out = build_system_prompt(
            &v,
            None,
            None,
            &ReleaseKind::Prerelease {
                label: "rc.1".into(),
            },
            VersionBump::Major,
            None,
        );
        let i_voice = out.find("v").unwrap();
        let i_bump = out.find("major release").unwrap();
        let i_pre = out.find("prerelease").unwrap();
        assert!(i_voice < i_bump);
        assert!(i_bump < i_pre);
    }

    #[test]
    fn calver_uses_calver_addendum() {
        let v = voice("v");
        let out = build_system_prompt(
            &v,
            None,
            None,
            &ReleaseKind::Stable,
            VersionBump::Major,
            Some(VersionScheme::Calver),
        );
        // CalVer year-line — should mention "year-line", *not* the
        // semver-major "breaking changes".
        assert!(out.contains("year-line"));
        assert!(!out.contains("breaking changes and migration"));
    }

    #[test]
    fn semver_uses_semver_addendum() {
        let v = voice("v");
        let out = build_system_prompt(
            &v,
            None,
            None,
            &ReleaseKind::Stable,
            VersionBump::Major,
            Some(VersionScheme::Semver),
        );
        assert!(out.contains("breaking changes and migration"));
        assert!(!out.contains("year-line"));
    }

    #[test]
    fn build_user_prompt_lists_sections_and_entries() {
        let classified = Classified(vec![
            entry(
                "drop legacy",
                Section::Breaking,
                ClassificationSource::Conventional,
            ),
            entry(
                "add login",
                Section::Features,
                ClassificationSource::BatchedLlm,
            ),
        ]);
        let prompt = build_user_prompt(
            &classified,
            Some("v0.1.0"),
            "HEAD",
            MergeStyle::Squash,
            &ReleaseKind::Stable,
            VersionBump::Patch,
            None,
            false,
        );
        assert!(prompt.contains("v0.1.0..HEAD"));
        assert!(prompt.contains("squash"));
        assert!(prompt.contains("Release kind: stable"));
        assert!(prompt.contains("Version bump: patch"));
        assert!(prompt.contains("## Breaking Changes"));
        assert!(prompt.contains("- drop legacy"));
        assert!(prompt.contains("[conv,"));
        assert!(prompt.contains("## Features"));
        assert!(prompt.contains("- add login"));
        assert!(prompt.contains("[llm-batch,"));
    }

    #[test]
    fn build_user_prompt_marks_prerelease_kind() {
        let classified = Classified::default();
        let prompt = build_user_prompt(
            &classified,
            None,
            "v1.0.0-rc.1",
            MergeStyle::Rebase,
            &ReleaseKind::Prerelease {
                label: "rc.1".into(),
            },
            VersionBump::Major,
            None,
            false,
        );
        assert!(prompt.contains("prerelease (rc.1)"));
        assert!(prompt.contains("Version bump: major"));
    }

    #[test]
    fn build_user_prompt_includes_pr_data_when_present() {
        let mut e = entry(
            "add login",
            Section::Features,
            ClassificationSource::BatchedLlm,
        );
        e.commit.pr_id = Some(42);
        e.pr = Some(PrInfo {
            number: 42,
            title: "Add SSO support".into(),
            body: "...".into(),
            labels: vec!["enhancement".into()],
            author: Some("ada".into()),
            merged_at: None,
            url: None,
        });
        let classified = Classified(vec![e]);
        let prompt = build_user_prompt(
            &classified,
            None,
            "HEAD",
            MergeStyle::Rebase,
            &ReleaseKind::Untagged,
            VersionBump::Unknown,
            None,
            false,
        );
        assert!(prompt.contains("(#42)"));
        assert!(prompt.contains("PR #42: Add SSO support"));
        assert!(prompt.contains("labels: enhancement"));
    }

    #[test]
    fn build_user_prompt_handles_empty() {
        let classified = Classified::default();
        let prompt = build_user_prompt(
            &classified,
            Some("v0.1.0"),
            "HEAD",
            MergeStyle::Rebase,
            &ReleaseKind::Stable,
            VersionBump::Patch,
            None,
            false,
        );
        assert!(prompt.contains("No commits in range"));
    }

    #[test]
    fn build_user_prompt_omits_bodies_when_rich_context_off() {
        let mut e = entry(
            "add login",
            Section::Features,
            ClassificationSource::BatchedLlm,
        );
        e.commit.body = "A meaningful commit body explaining the change.".into();
        e.pr = Some(PrInfo {
            number: 7,
            title: "Add login".into(),
            body: "PR description with rationale.".into(),
            labels: vec![],
            author: None,
            merged_at: None,
            url: None,
        });
        let classified = Classified(vec![e]);
        let prompt = build_user_prompt(
            &classified,
            None,
            "HEAD",
            MergeStyle::Rebase,
            &ReleaseKind::Untagged,
            VersionBump::Unknown,
            None,
            false,
        );
        assert!(!prompt.contains("commit-body:"));
        assert!(!prompt.contains("pr-body:"));
        assert!(!prompt.contains("meaningful commit body"));
    }

    #[test]
    fn build_user_prompt_includes_bodies_when_rich_context_on() {
        let mut e = entry(
            "add login",
            Section::Features,
            ClassificationSource::BatchedLlm,
        );
        e.commit.body = "A meaningful commit body explaining the change.".into();
        e.pr = Some(PrInfo {
            number: 7,
            title: "Add login".into(),
            body: "PR description with rationale.".into(),
            labels: vec![],
            author: None,
            merged_at: None,
            url: None,
        });
        let classified = Classified(vec![e]);
        let prompt = build_user_prompt(
            &classified,
            None,
            "HEAD",
            MergeStyle::Rebase,
            &ReleaseKind::Untagged,
            VersionBump::Unknown,
            None,
            true,
        );
        assert!(prompt.contains("commit-body:"));
        assert!(prompt.contains("A meaningful commit body explaining the change."));
        assert!(prompt.contains("pr-body:"));
        assert!(prompt.contains("PR description with rationale."));
    }

    #[test]
    fn build_user_prompt_skips_empty_bodies_under_rich_context() {
        // Body is empty; commit has no PR. Rich context should not emit
        // dangling labels or trailing whitespace lines.
        let e = entry("tweak", Section::Other, ClassificationSource::Default);
        let classified = Classified(vec![e]);
        let prompt = build_user_prompt(
            &classified,
            None,
            "HEAD",
            MergeStyle::Rebase,
            &ReleaseKind::Untagged,
            VersionBump::Unknown,
            None,
            true,
        );
        assert!(!prompt.contains("commit-body:"));
        assert!(!prompt.contains("pr-body:"));
    }

    #[test]
    fn build_user_prompt_truncates_long_bodies() {
        let mut e = entry(
            "epic change",
            Section::Features,
            ClassificationSource::BatchedLlm,
        );
        // 2× the commit-body cap (600) so we can verify truncation.
        e.commit.body = "x".repeat(COMMIT_BODY_PREVIEW * 2);
        let classified = Classified(vec![e]);
        let prompt = build_user_prompt(
            &classified,
            None,
            "HEAD",
            MergeStyle::Rebase,
            &ReleaseKind::Untagged,
            VersionBump::Unknown,
            None,
            true,
        );
        // The body block exists, contains an ellipsis, and the prompt as
        // a whole stays well below 2× the cap (i.e. truncation actually
        // bit).
        assert!(prompt.contains("commit-body:"));
        assert!(prompt.contains("…"));
        assert!(prompt.len() < COMMIT_BODY_PREVIEW * 2);
    }

    #[test]
    fn build_user_prompt_omits_from_when_none() {
        let classified = Classified::default();
        let prompt = build_user_prompt(
            &classified,
            None,
            "HEAD",
            MergeStyle::Rebase,
            &ReleaseKind::Untagged,
            VersionBump::Unknown,
            None,
            false,
        );
        assert!(prompt.contains("`HEAD`"));
        assert!(!prompt.contains(".."));
    }
}
