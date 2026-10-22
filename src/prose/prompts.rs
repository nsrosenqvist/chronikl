//! Prompt assembly for the prose pass.
//!
//! Builds the system + user prompts the prose model sees. The system
//! prompt is the loaded voice plus any addenda; the user prompt is a
//! structured listing of the classified commits the model should turn
//! into prose.

use crate::constants::NOTHING_USER_FACING_TOKEN;
use crate::models::{
    Classified, ClassifiedCommit, MergeStyle, ReleaseKind, RenderSettings, TextFormat, VersionBump,
    VersionScheme,
};
use crate::project::ProjectContext;
use crate::voice::Voice;

/// Build the system prompt for the prose pass: the voice's body, plus
/// any extra instructions appended after a blank line.
///
/// `extra_instructions` typically comes from `voice.extra_instructions`
/// in the TOML config. `inline_prompt` is the `--prompt "<TEXT>"` CLI
/// flag — applied last so a one-shot override always wins.
///
/// `render` decides two more addenda: user-facing guidance when
/// `user_facing` is on, and an "Output format" section whenever
/// the run asks for a shape other than sectioned Markdown (flat list,
/// limits, plain text, no footer). The format section restates exactly
/// what the user message asks for, so the two never disagree. The
/// bump and prerelease framings describe a sectioned developer
/// document (overview paragraph, migration notes), so they're only
/// added for developer notes with headings on.
#[allow(clippy::too_many_arguments)]
pub fn build_system_prompt(
    voice: &Voice,
    extra_instructions: Option<&str>,
    inline_prompt: Option<&str>,
    release_kind: &ReleaseKind,
    bump: VersionBump,
    scheme: Option<VersionScheme>,
    render: &RenderSettings,
) -> String {
    let mut out = voice.system_prompt.clone();

    // Auto-addenda are appended *between* the voice and the
    // user-supplied instructions so the user's `--prompt` always wins
    // when there's a conflict (e.g. "ignore the framing, this is
    // internal"). Order: voice → bump → prerelease → user-facing →
    // output format → user extras.
    let developer_document = !render.user_facing && render.headings;
    if developer_document {
        if let Some(addendum) = crate::prompts::bump_addendum(bump, scheme) {
            out.push_str("\n\n");
            out.push_str(addendum);
        }
        if let ReleaseKind::Prerelease { label } = release_kind {
            out.push_str("\n\n");
            out.push_str(&crate::prompts::prerelease_addendum(label));
        }
    }
    if render.user_facing {
        out.push_str("\n\n");
        out.push_str(crate::prompts::USER_FACING.trim());
    }
    if let Some(format) = format_addendum(render) {
        out.push_str("\n\n");
        out.push_str(&format);
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

/// The format rules a run's settings impose, one per line, worded the
/// same way for the system prompt and the user message. Empty for the
/// default shape (sectioned Markdown, no limits, footer on).
fn format_rules(render: &RenderSettings) -> Vec<String> {
    let mut rules = Vec::new();
    if !render.headings {
        rules.push(
            "Write one flat list of bullets, each starting with `- `, most noticeable change \
             first. No headings, no section names, no overview paragraph and no closing line."
                .to_string(),
        );
    } else if render.format == TextFormat::Plain {
        rules.push(
            "Put each section name on its own line with no `#` marks, followed by its bullets, \
             each starting with `- `. Skip empty sections."
                .to_string(),
        );
    }
    if let Some(n) = render.max_bullets {
        rules.push(format!(
            "Write at most {n} bullet{}. Merge or drop the least noticeable items to fit.",
            if n == 1 { "" } else { "s" }
        ));
    }
    if let Some(n) = render.max_chars {
        rules.push(format!(
            "Keep the whole text within {n} characters, counting bullet markers and line \
             breaks. Prefer fewer, shorter bullets; anything past the limit is cut at whole \
             bullets."
        ));
    }
    if render.format == TextFormat::Plain {
        rules.push(
            "Write plain text, not Markdown: no bold, italics, code spans, links or `#` headings."
                .to_string(),
        );
    }
    if !render.footer {
        rules.push("Don't add a changelog link, a compare link or any other trailer.".to_string());
    }
    if render.user_facing {
        rules.push(format!(
            "If nothing in the list is something a user would notice, output exactly \
             `{NOTHING_USER_FACING_TOKEN}` and nothing else."
        ));
    }
    rules
}

/// System-prompt section stating the run's format rules, or `None` for
/// the default shape.
fn format_addendum(render: &RenderSettings) -> Option<String> {
    let rules = format_rules(render);
    if rules.is_empty() {
        return None;
    }
    let mut out = String::from(
        "# Output format\n\nThese rules come from this run's settings and take precedence over \
         any document-shape guidance above:\n\n",
    );
    for rule in rules {
        out.push_str("- ");
        out.push_str(&rule);
        out.push('\n');
    }
    Some(out.trim_end().to_string())
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
/// Classifier-provenance fields (source tier, confidence) are kept out
/// of the user prompt — they're audit-log material, and including them
/// here historically caused the writer to surface them as fake `(#…)`
/// references when no real PR ID was available.
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
/// - Drop support for Node 18  (#42)
///   PR #42: Remove legacy runtime  labels: breaking-change
///
/// ## Features
/// - Add login flow
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
    project: &ProjectContext,
    render: &RenderSettings,
) -> String {
    let mut out = String::new();
    push_project_context(&mut out, project);
    let count = classified.len();
    let range_str = match from_ref {
        Some(from) => format!("{from}..{to_ref}"),
        None => to_ref.to_string(),
    };
    if matches!(bump, VersionBump::Initial) {
        out.push_str(&format!(
            "Generate the initial-release announcement. There is no prior \
             version of this project. The commits below (range: `{range_str}`, \
             {count} commits) are the work that *prepared* this release — they \
             are not additions to a pre-existing product. Treat them as \
             evidence of what the project includes, and frame the announcement \
             as a description of what the project *is*.\n\n"
        ));
    } else {
        out.push_str(&format!(
            "Generate release notes for the range `{range_str}` ({count} commits).\n\n"
        ));
    }
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

    if render.user_facing {
        out.push_str("Audience: people who use the product, not its developers.\n\n");
    }

    // With headings on, the groups are written as Markdown headers the
    // model reuses. With headings off they're plain labels, and the
    // closing instruction says not to reproduce them.
    if render.headings {
        out.push_str("Commits, grouped by section:\n\n");
    } else {
        out.push_str("Commits, grouped by kind (the kinds are context, not headings):\n\n");
    }
    for (section, entries) in classified.group_by_section() {
        if render.headings {
            out.push_str(&format!("## {}\n", section.header()));
        } else {
            out.push_str(&format!("{}:\n", section.header()));
        }
        for entry in entries {
            push_entry(&mut out, entry, rich_context);
        }
        out.push('\n');
    }

    let rules = format_rules(render);
    if rules.is_empty() {
        out.push_str(
            "Write release notes in your configured voice. Output GitHub-flavored \
             Markdown only. Use the section names above as `### Section Name` \
             headers and skip empty sections. Do not invent entries that aren't \
             in the list above. Do not include a top-level title or a \
             `## What's Changed` wrapper.\n",
        );
        return out;
    }

    out.push_str("Write release notes in your configured voice.");
    if render.headings && render.format == TextFormat::Markdown {
        out.push_str(
            " Output GitHub-flavored Markdown. Use the section names above as \
             `### Section Name` headers and skip empty sections.",
        );
    }
    out.push_str(
        " Do not invent entries that aren't in the list above. Do not include \
         a top-level title.",
    );
    if render.user_facing {
        out.push_str(
            " Leave out items that are internal to the project's development even \
             when they are listed above (tests, docs, CI, builds, tooling, \
             refactors, dependency updates) and features only staff, testers or \
             app-store reviewers use, and describe the rest in plain words an \
             ordinary user understands.",
        );
    }
    out.push_str("\n\n");
    for rule in rules {
        out.push_str("- ");
        out.push_str(&rule);
        out.push('\n');
    }
    out
}

/// Prepend a "Project context:" block when any field is present. Goes
/// before the range-opener so the model has "what this project is"
/// before it reads the commit list. Silently no-op when the context is
/// empty, so callers don't need to branch.
fn push_project_context(out: &mut String, project: &ProjectContext) {
    if project.is_empty() {
        return;
    }
    out.push_str("Project context:\n\n");
    if let Some(desc) = &project.description {
        out.push_str(&format!("- Description: {desc}\n"));
    }
    if let Some(intro) = &project.readme_intro {
        out.push_str("- README intro:\n");
        for line in intro.lines() {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push('\n');
}

fn push_entry(out: &mut String, entry: &ClassifiedCommit, rich_context: bool) {
    let summary = entry.classification.summary.trim();
    out.push_str(&format!("- {summary}"));
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
            render: Default::default(),
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
                None,
                &RenderSettings::default()
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
            &RenderSettings::default(),
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
            &RenderSettings::default(),
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
            &RenderSettings::default(),
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
            &RenderSettings::default(),
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
            &RenderSettings::default(),
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
            &RenderSettings::default(),
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
            &RenderSettings::default(),
        );
        assert!(patch.contains("patch release"));

        let initial = build_system_prompt(
            &v,
            None,
            None,
            &ReleaseKind::Stable,
            VersionBump::Initial,
            None,
            &RenderSettings::default(),
        );
        assert!(initial.contains("initial public release"));

        let unknown = build_system_prompt(
            &v,
            None,
            None,
            &ReleaseKind::Stable,
            VersionBump::Unknown,
            None,
            &RenderSettings::default(),
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
            &RenderSettings::default(),
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
            &RenderSettings::default(),
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
            &RenderSettings::default(),
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
            &ProjectContext::default(),
            &RenderSettings::default(),
        );
        assert!(prompt.contains("v0.1.0..HEAD"));
        assert!(prompt.contains("squash"));
        assert!(prompt.contains("Release kind: stable"));
        assert!(prompt.contains("Version bump: patch"));
        assert!(prompt.contains("## Breaking Changes"));
        assert!(prompt.contains("- drop legacy"));
        assert!(prompt.contains("## Features"));
        assert!(prompt.contains("- add login"));
        // Classifier-provenance must NOT leak into the user prompt:
        // historically `[conv, conf=1.0]` / `[llm-batch, conf=0.6]` were
        // appended after each summary, and writer models surfaced the
        // tags as fake `(#conv)` references when no real PR ID existed.
        assert!(!prompt.contains("[conv"));
        assert!(!prompt.contains("[llm-batch"));
        assert!(!prompt.contains("conf="));
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
            &ProjectContext::default(),
            &RenderSettings::default(),
        );
        assert!(prompt.contains("prerelease (rc.1)"));
        assert!(prompt.contains("Version bump: major"));
    }

    #[test]
    fn push_entry_omits_classifier_provenance() {
        // Regression: source tier ('conv', 'llm-batch', …) and confidence
        // must not appear in the user prompt. They used to be appended as
        // `[source, conf=N]` and writer models occasionally surfaced the
        // tags as fake `(#conv)` references when no real PR ID was
        // present.
        let e = entry(
            "add login",
            Section::Features,
            ClassificationSource::BatchedLlm,
        );
        let mut out = String::new();
        push_entry(&mut out, &e, false);
        assert_eq!(out, "- add login\n");
    }

    #[test]
    fn push_entry_appends_pr_id_when_present() {
        let mut e = entry(
            "add login",
            Section::Features,
            ClassificationSource::BatchedLlm,
        );
        e.commit.pr_id = Some(42);
        let mut out = String::new();
        push_entry(&mut out, &e, false);
        assert_eq!(out, "- add login  (#42)\n");
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
            &ProjectContext::default(),
            &RenderSettings::default(),
        );
        assert!(prompt.contains("(#42)"));
        assert!(prompt.contains("PR #42: Add SSO support"));
        assert!(prompt.contains("labels: enhancement"));
    }

    #[test]
    fn build_user_prompt_uses_initial_release_opener_on_initial_bump() {
        let classified = Classified(vec![entry(
            "wire up auth",
            Section::Features,
            ClassificationSource::Conventional,
        )]);
        let prompt = build_user_prompt(
            &classified,
            Some("684e57a"),
            "v1.0.0",
            MergeStyle::Rebase,
            &ReleaseKind::Stable,
            VersionBump::Initial,
            None,
            false,
            &ProjectContext::default(),
            &RenderSettings::default(),
        );
        assert!(prompt.contains("initial-release announcement"));
        assert!(prompt.contains("no prior version"));
        assert!(prompt.contains("not additions to a pre-existing product"));
        // The generic delta opener must not appear for initial releases.
        assert!(!prompt.contains("Generate release notes for the range"));
    }

    #[test]
    fn build_user_prompt_uses_generic_opener_for_non_initial_bumps() {
        let classified = Classified(vec![entry(
            "fix nav",
            Section::BugFixes,
            ClassificationSource::Conventional,
        )]);
        let prompt = build_user_prompt(
            &classified,
            Some("v1.0.0"),
            "v1.0.1",
            MergeStyle::Rebase,
            &ReleaseKind::Stable,
            VersionBump::Patch,
            None,
            false,
            &ProjectContext::default(),
            &RenderSettings::default(),
        );
        assert!(prompt.contains("Generate release notes for the range"));
        assert!(!prompt.contains("initial-release announcement"));
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
            &ProjectContext::default(),
            &RenderSettings::default(),
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
            &ProjectContext::default(),
            &RenderSettings::default(),
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
            &ProjectContext::default(),
            &RenderSettings::default(),
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
            &ProjectContext::default(),
            &RenderSettings::default(),
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
            &ProjectContext::default(),
            &RenderSettings::default(),
        );
        // The body block exists, contains an ellipsis, and the prompt as
        // a whole stays well below 2× the cap (i.e. truncation actually
        // bit).
        assert!(prompt.contains("commit-body:"));
        assert!(prompt.contains("…"));
        assert!(prompt.len() < COMMIT_BODY_PREVIEW * 2);
    }

    #[test]
    fn build_user_prompt_renders_project_context_block_when_present() {
        let classified = Classified(vec![entry(
            "wire up auth",
            Section::Features,
            ClassificationSource::Conventional,
        )]);
        let project = ProjectContext {
            description: Some("AI-powered release notes generator".into()),
            readme_intro: Some("# chronikl\n\nTurn commits into prose.".into()),
        };
        let prompt = build_user_prompt(
            &classified,
            Some("684e57a"),
            "v1.0.0",
            MergeStyle::Rebase,
            &ReleaseKind::Stable,
            VersionBump::Initial,
            None,
            false,
            &project,
            &RenderSettings::default(),
        );
        // Project block must come before the range opener so the model
        // reads "what this project is" first.
        let i_project = prompt.find("Project context:").unwrap();
        let i_opener = prompt.find("initial-release announcement").unwrap();
        assert!(i_project < i_opener);
        assert!(prompt.contains("- Description: AI-powered release notes generator"));
        assert!(prompt.contains("- README intro:"));
        assert!(prompt.contains("# chronikl"));
        assert!(prompt.contains("Turn commits into prose."));
    }

    #[test]
    fn build_user_prompt_omits_project_context_block_when_empty() {
        let classified = Classified(vec![entry(
            "add login",
            Section::Features,
            ClassificationSource::Conventional,
        )]);
        let prompt = build_user_prompt(
            &classified,
            Some("v0.1.0"),
            "v0.1.1",
            MergeStyle::Rebase,
            &ReleaseKind::Stable,
            VersionBump::Patch,
            None,
            false,
            &ProjectContext::default(),
            &RenderSettings::default(),
        );
        assert!(!prompt.contains("Project context:"));
        // The range opener should still be the first line.
        assert!(
            prompt
                .trim_start()
                .starts_with("Generate release notes for the range")
        );
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
            &ProjectContext::default(),
            &RenderSettings::default(),
        );
        assert!(prompt.contains("`HEAD`"));
        assert!(!prompt.contains(".."));
    }

    fn end_user_render() -> RenderSettings {
        RenderSettings {
            user_facing: true,
            headings: false,
            footer: false,
            max_bullets: Some(5),
            max_chars: Some(450),
            format: TextFormat::Plain,
            ..RenderSettings::default()
        }
    }

    fn two_sections() -> Classified {
        Classified(vec![
            entry(
                "add search",
                Section::Features,
                ClassificationSource::Conventional,
            ),
            entry(
                "fix crash on start",
                Section::BugFixes,
                ClassificationSource::Conventional,
            ),
        ])
    }

    fn user_prompt(render: &RenderSettings) -> String {
        build_user_prompt(
            &two_sections(),
            Some("v1.0.0"),
            "v1.1.0",
            MergeStyle::Rebase,
            &ReleaseKind::Stable,
            VersionBump::Minor,
            None,
            false,
            &ProjectContext::default(),
            render,
        )
    }

    fn system_prompt(render: &RenderSettings) -> String {
        build_system_prompt(
            &voice("voice body"),
            None,
            Some("inline last"),
            &ReleaseKind::Prerelease {
                label: "rc.1".into(),
            },
            VersionBump::Minor,
            None,
            render,
        )
    }

    #[test]
    fn default_render_keeps_the_sectioned_instruction() {
        let prompt = user_prompt(&RenderSettings::default());
        assert!(prompt.contains("## Features\n- add search"));
        assert!(prompt.contains("`### Section Name`"));
        assert!(!prompt.contains("flat list"));
        let system = system_prompt(&RenderSettings::default());
        assert!(!system.contains("# Output format"));
        assert!(system.contains("minor release"));
        assert!(system.contains("prerelease (rc.1)"));
    }

    #[test]
    fn headings_off_never_asks_for_section_headers() {
        let render = RenderSettings {
            headings: false,
            ..RenderSettings::default()
        };
        let prompt = user_prompt(&render);
        assert!(!prompt.contains("### Section Name"), "{prompt}");
        assert!(!prompt.contains("## Features"), "{prompt}");
        assert!(prompt.contains("Features:\n- add search"));
        assert!(prompt.contains("one flat list of bullets"));
        assert!(prompt.contains("No headings"));
        // The system prompt states the same rule and drops the
        // document-shaped framings.
        let system = system_prompt(&render);
        assert!(system.contains("# Output format"));
        assert!(system.contains("one flat list of bullets"));
        assert!(!system.contains("minor release"));
        assert!(!system.contains("Prerelease Notes"));
    }

    #[test]
    fn limits_and_format_appear_in_both_prompts() {
        let render = end_user_render();
        let prompt = user_prompt(&render);
        let system = system_prompt(&render);
        for text in [&prompt, &system] {
            assert!(text.contains("at most 5 bullets"), "{text}");
            assert!(text.contains("within 450 characters"), "{text}");
            assert!(text.contains("plain text, not Markdown"), "{text}");
            assert!(text.contains("changelog link"), "{text}");
        }
    }

    #[test]
    fn user_facing_notes_filter_in_the_prose_pass() {
        let render = end_user_render();
        let prompt = user_prompt(&render);
        assert!(prompt.contains("Audience: people who use the product"));
        assert!(prompt.contains("Leave out items that are internal"));
        assert!(prompt.contains("even when"));
        let system = system_prompt(&render);
        assert!(system.contains("# User-facing notes"));
        assert!(system.contains("even when the input lists it as a"));
        for text in [&prompt, &system] {
            assert!(text.contains(NOTHING_USER_FACING_TOKEN), "{text}");
        }
        // The bump and prerelease framings are for developer documents.
        assert!(!system.contains("minor release"));
        // User extras still come last.
        let i_user_facing = system.find("# User-facing notes").unwrap();
        let i_format = system.find("# Output format").unwrap();
        let i_inline = system.find("inline last").unwrap();
        assert!(i_user_facing < i_format && i_format < i_inline);
    }

    #[test]
    fn single_bullet_limit_reads_naturally() {
        let render = RenderSettings {
            max_bullets: Some(1),
            ..RenderSettings::default()
        };
        assert!(user_prompt(&render).contains("at most 1 bullet."));
    }

    #[test]
    fn plain_with_headings_asks_for_bare_section_names() {
        let render = RenderSettings {
            format: TextFormat::Plain,
            ..RenderSettings::default()
        };
        let prompt = user_prompt(&render);
        assert!(prompt.contains("section name on its own line with no `#` marks"));
        assert!(!prompt.contains("`### Section Name`"));
    }

    #[test]
    fn simple_voice_prompts_follow_its_frontmatter_and_terse_stays_sectioned() {
        let cfg = crate::config::Config::default();
        let simple = crate::voice::load_bundled("simple").unwrap();
        let render = cfg.render_settings(&simple.render).unwrap();
        let prompt = user_prompt(&render);
        assert!(!prompt.contains("### Section Name"), "{prompt}");
        assert!(prompt.contains("one flat list of bullets"));
        assert!(prompt.contains("plain text, not Markdown"));
        assert!(prompt.contains(NOTHING_USER_FACING_TOKEN));

        let terse = crate::voice::load_bundled("terse").unwrap();
        let render = cfg.render_settings(&terse.render).unwrap();
        assert_eq!(render, RenderSettings::default());
        let prompt = user_prompt(&render);
        assert!(prompt.contains("`### Section Name`"));
        assert!(!prompt.contains(NOTHING_USER_FACING_TOKEN));
    }
}
