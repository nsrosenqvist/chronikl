//! Prose pass — turns the classified commits into human-friendly
//! release-notes Markdown using the configured voice.
//!
//! Single-call by default: one provider invocation produces the whole
//! document. The deterministic Markdown renderer remains the fallback
//! when an LLM call isn't possible (`--no-llm`, no provider configured,
//! or a runtime failure).

pub mod prompts;

use crate::audit::AuditSink;
use crate::constants::PROSE_MAX_OUTPUT_TOKENS;
use crate::models::{
    Classified, MergeStyle, ReleaseKind, RenderSettings, VersionBump, VersionScheme,
};
use crate::project::ProjectContext;
use crate::providers::{CompletionRequest, NotesProvider, ProviderError};
use crate::voice::Voice;

/// Inputs for [`run`]. Borrowed so callers don't pay for clones in the
/// happy path.
#[derive(Debug)]
pub struct ProseRequest<'a> {
    pub voice: &'a Voice,
    pub extra_instructions: Option<&'a str>,
    pub inline_prompt: Option<&'a str>,
    pub classified: &'a Classified,
    pub merge_style: MergeStyle,
    pub release_kind: &'a ReleaseKind,
    pub version_bump: VersionBump,
    pub version_scheme: Option<VersionScheme>,
    pub from_ref: Option<&'a str>,
    pub to_ref: &'a str,
    /// When `true`, the user prompt embeds truncated commit bodies and
    /// PR bodies under each entry. Off by default; opt-in via
    /// `--rich-context` or `[voice].rich_context = true`.
    pub rich_context: bool,
    /// Optional project metadata (description, README intro) folded
    /// into the user prompt as a "Project context:" block. Auto-detected
    /// from `Cargo.toml` / `package.json` / `pyproject.toml` and the
    /// repo's README by default; configurable via `[project]` in TOML
    /// or `--project-description` / `--readme` / `--no-readme`.
    pub project_context: &'a ProjectContext,
    /// Who the notes are for and their shape. The prompts ask for exactly this
    /// shape; [`crate::output::shape`] enforces it on the result.
    pub render: &'a RenderSettings,
}

/// Run the prose pass. Returns the Markdown text produced by the model.
///
/// Tags the audit-log entry with all commit SHAs included in the
/// classified set, so the trail records "this prose pass covered these
/// commits" without breaking the per-call hash convention.
pub async fn run(
    request: ProseRequest<'_>,
    provider: &dyn NotesProvider,
    _audit: &AuditSink,
) -> Result<String, ProviderError> {
    let system_prompt = prompts::build_system_prompt(
        request.voice,
        request.extra_instructions,
        request.inline_prompt,
        request.release_kind,
        request.version_bump,
        request.version_scheme,
        request.render,
    );
    let user_prompt = prompts::build_user_prompt(
        request.classified,
        request.from_ref,
        request.to_ref,
        request.merge_style,
        request.release_kind,
        request.version_bump,
        request.version_scheme,
        request.rich_context,
        request.project_context,
        request.render,
    );

    let shas: Vec<String> = request
        .classified
        .iter()
        .map(|c| c.commit.sha.clone())
        .collect();

    let response = provider
        .complete(CompletionRequest {
            system_prompt: &system_prompt,
            user_prompt: &user_prompt,
            // Prose is free-form Markdown; no structured-output schema.
            schema: serde_json::Value::Null,
            max_tokens: PROSE_MAX_OUTPUT_TOKENS,
            label: "prose",
            commit_shas: &shas,
        })
        .await?;

    let cleaned = postprocess(&response.text);
    Ok(cleaned)
}

/// What the prose model said, once its "nothing user-facing" answer is
/// recognized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProseOutcome {
    /// Notes to publish (any stray token lines removed).
    Notes(String),
    /// The model found nothing a user would notice.
    NothingUserFacing,
}

/// Recognize the [`crate::constants::NOTHING_USER_FACING_TOKEN`] answer.
/// Lines that are only the token (with or without a bullet marker, code
/// span or period) are dropped; when nothing else is left, the model
/// meant "nothing user-facing".
pub fn interpret(text: &str) -> ProseOutcome {
    let token = crate::constants::NOTHING_USER_FACING_TOKEN;
    let is_token = |line: &str| {
        let t = line
            .trim()
            .trim_start_matches(['-', '*', '•'])
            .trim()
            .trim_matches(['`', '*', '_', '.', '"'])
            .trim();
        t.eq_ignore_ascii_case(token)
    };
    if !text.lines().any(is_token) {
        return ProseOutcome::Notes(text.to_string());
    }
    let kept: Vec<&str> = text.lines().filter(|l| !is_token(l)).collect();
    let kept = kept.join("\n");
    if kept.trim().is_empty() {
        ProseOutcome::NothingUserFacing
    } else {
        ProseOutcome::Notes(kept.trim().to_string())
    }
}

/// Light post-processing on the model's Markdown:
/// - Trim trailing whitespace.
/// - Strip a leading ` ```markdown ... ``` ` fence if the model wrapped
///   the whole document in one (some providers do this even with an
///   explicit "no code fences" instruction).
fn postprocess(s: &str) -> String {
    let trimmed = s.trim();
    if let Some(stripped) = strip_full_fence(trimmed) {
        return stripped.trim().to_string();
    }
    trimmed.to_string()
}

fn strip_full_fence(s: &str) -> Option<String> {
    let trimmed = s.trim();
    let opening = if let Some(rest) = trimmed.strip_prefix("```markdown\n") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("```md\n") {
        rest
    } else {
        trimmed.strip_prefix("```\n")?
    };
    opening.strip_suffix("```").map(|inner| inner.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpret_recognizes_the_nothing_token() {
        for text in [
            "NO_USER_FACING_CHANGES",
            "- NO_USER_FACING_CHANGES",
            "`NO_USER_FACING_CHANGES`.",
            "\n no_user_facing_changes \n",
        ] {
            assert_eq!(interpret(text), ProseOutcome::NothingUserFacing, "{text}");
        }
    }

    #[test]
    fn interpret_keeps_notes_and_drops_stray_tokens() {
        assert_eq!(
            interpret("- Faster search"),
            ProseOutcome::Notes("- Faster search".into())
        );
        assert_eq!(
            interpret("- Faster search\n- NO_USER_FACING_CHANGES"),
            ProseOutcome::Notes("- Faster search".into())
        );
        // A bullet that merely mentions the token is kept.
        let text = "- Explain NO_USER_FACING_CHANGES in the docs";
        assert_eq!(interpret(text), ProseOutcome::Notes(text.into()));
    }

    #[test]
    fn postprocess_trims_outer_whitespace() {
        assert_eq!(postprocess("  \n# Hi\n"), "# Hi");
    }

    #[test]
    fn postprocess_strips_markdown_fence() {
        let raw = "```markdown\n# Release notes\n\n### Features\n- Add x\n```";
        let out = postprocess(raw);
        assert!(out.starts_with("# Release notes"));
        assert!(!out.contains("```"));
    }

    #[test]
    fn postprocess_strips_md_fence_alias() {
        let raw = "```md\n## Hi\n```";
        let out = postprocess(raw);
        assert_eq!(out, "## Hi");
    }

    #[test]
    fn postprocess_strips_unlabelled_fence() {
        let raw = "```\n## Hi\n```";
        let out = postprocess(raw);
        assert_eq!(out, "## Hi");
    }

    #[test]
    fn postprocess_keeps_inner_fences() {
        // Inner code fences should be preserved.
        let raw = "## Hi\n\nSee `inline` and:\n\n```rust\nfn x() {}\n```\n\nMore text.";
        let out = postprocess(raw);
        assert!(out.contains("```rust"));
    }
}
