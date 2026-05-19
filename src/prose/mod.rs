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
use crate::models::{Classified, MergeStyle, ReleaseKind, VersionBump, VersionScheme};
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
