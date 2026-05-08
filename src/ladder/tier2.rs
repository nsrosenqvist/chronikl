//! Tier 2 — per-commit LLM classification with diff context.
//!
//! For each commit whose Tier 0/1 confidence is still below the
//! configured threshold, fetch the per-commit diff, truncate it to fit
//! the token budget, and ask the model to classify with the diff as
//! ground truth. The model can also flag breaking changes it discovers
//! by reading the diff (e.g. a removed public function whose
//! commit message didn't mention it).
//!
//! Per-commit calls run with bounded concurrency
//! (`crate::constants::LLM_CONCURRENCY`) so the network round-trips
//! amortize without exceeding provider rate limits.

use std::path::Path;

use futures::stream::{StreamExt, TryStreamExt};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::audit::AuditSink;
use crate::git;
use crate::models::{Classification, ClassificationSource, Classified, Section};
use crate::providers::response::parse_with_fallbacks;
use crate::providers::{CompletionRequest, NotesProvider, ProviderError};

use crate::constants::{LLM_CONCURRENCY, TIER2_MAX_OUTPUT_TOKENS};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Tier2Verdict {
    pub section: String,
    pub summary: String,
    pub confidence: f32,
    /// `true` if the diff reveals a breaking change. When set, the
    /// classification is forced to [`Section::Breaking`] regardless of
    /// `section`.
    #[serde(default)]
    pub breaking: bool,
}

/// Run Tier 2 over a [`Classified`] in-place.
///
/// Returns the number of commits whose classification was updated.
pub async fn classify(
    classified: &mut Classified,
    repo: &Path,
    provider: &dyn NotesProvider,
    _audit: &AuditSink,
    max_diff_tokens: usize,
    confidence_threshold: f32,
    system_prompt: &str,
) -> Result<usize, ProviderError> {
    let pending: Vec<usize> = classified
        .0
        .iter()
        .enumerate()
        .filter(|(_, c)| c.classification.confidence < confidence_threshold)
        .map(|(i, _)| i)
        .collect();

    if pending.is_empty() {
        return Ok(0);
    }

    let schema = serde_json::to_value(schema_for!(Tier2Verdict)).unwrap_or(serde_json::Value::Null);

    // Build per-commit jobs upfront so the futures own everything they
    // need (prompt, sha, diff) and don't borrow `classified`.
    struct Job {
        global_index: usize,
        sha: [String; 1],
        user_prompt: String,
    }
    let mut jobs: Vec<Job> = Vec::with_capacity(pending.len());
    for global_index in pending {
        let entry = &classified.0[global_index];
        let diff_raw = git::commit_diff(repo, &entry.commit.sha)
            .await
            .map_err(|e| ProviderError::Api(format!("git show failed: {e}")))?;
        let diff = truncate_diff(&diff_raw, max_diff_tokens);
        jobs.push(Job {
            global_index,
            sha: [entry.commit.sha.clone()],
            user_prompt: build_prompt(entry, &diff),
        });
    }

    let label = "tier-2-per-commit";
    let results: Vec<(usize, Tier2Verdict)> = futures::stream::iter(jobs.iter().map(|job| {
        let schema = schema.clone();
        async move {
            let response = provider
                .complete(CompletionRequest {
                    system_prompt,
                    user_prompt: &job.user_prompt,
                    schema,
                    max_tokens: TIER2_MAX_OUTPUT_TOKENS,
                    label,
                    commit_shas: &job.sha,
                })
                .await?;
            let verdict = parse_with_fallbacks::<Tier2Verdict>(&response.text)?;
            Ok::<_, ProviderError>((job.global_index, verdict))
        }
    }))
    .buffer_unordered(LLM_CONCURRENCY)
    .try_collect()
    .await?;

    let mut updated = 0usize;
    for (global_index, verdict) in results {
        let section = if verdict.breaking {
            Section::Breaking
        } else {
            Section::parse_lenient(&verdict.section).unwrap_or(Section::Other)
        };
        classified.0[global_index].classification = Classification {
            section,
            summary: verdict.summary.trim().to_string(),
            source: ClassificationSource::PerCommitLlm,
            confidence: verdict.confidence.clamp(0.0, 1.0),
        };
        updated += 1;
    }
    Ok(updated)
}

/// Default Tier 2 system prompt. Loaded from `src/prompts/tier2_system.md`.
pub fn default_system_prompt() -> &'static str {
    crate::prompts::TIER2_SYSTEM
}

/// Estimate token count from byte length. ~1 token / 4 bytes is a
/// well-known heuristic that errs slightly on the high side, which
/// makes truncation conservative.
pub fn estimate_tokens(s: &str) -> usize {
    s.len() / 4
}

/// Truncate a diff to fit a token budget. If the diff fits, return
/// verbatim; otherwise keep as much of the leading content as possible
/// (file headers come first in `diff --git` output) and append a
/// truncation marker noting the original size.
pub fn truncate_diff(diff: &str, max_tokens: usize) -> String {
    let estimated = estimate_tokens(diff);
    if estimated <= max_tokens {
        return diff.to_string();
    }
    let max_bytes = max_tokens * 4;
    let mut out = String::with_capacity(max_bytes + 200);
    let mut byte_count = 0usize;
    for line in diff.lines() {
        // +1 for the newline we re-add.
        if byte_count + line.len() + 1 > max_bytes {
            break;
        }
        out.push_str(line);
        out.push('\n');
        byte_count += line.len() + 1;
    }
    out.push_str(&format!(
        "\n[…diff truncated to fit ~{max_tokens}-token budget; original was \
         ~{estimated} tokens]\n"
    ));
    out
}

/// Public for the `chronikl debug prompts` subcommand. Builds the
/// exact user prompt Tier 2 would send for a given commit + diff.
pub fn debug_prompt(entry: &crate::models::ClassifiedCommit, diff: &str) -> String {
    build_prompt(entry, diff)
}

fn build_prompt(entry: &crate::models::ClassifiedCommit, diff: &str) -> String {
    let mut out = crate::prompts::commit_context_prompt(entry, diff);
    out.push_str(
        "\n\nReturn JSON: {\"section\": \"...\", \"summary\": \"...\", \
         \"confidence\": 0.0, \"breaking\": false}.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_basic() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }

    #[test]
    fn truncate_diff_passthrough_when_under_budget() {
        let diff = "diff --git a/x b/x\n@@ -1 +1 @@\n-old\n+new\n";
        let out = truncate_diff(diff, 1000);
        assert_eq!(out, diff);
    }

    #[test]
    fn truncate_diff_appends_marker_when_over_budget() {
        let diff = "x".repeat(10_000);
        let out = truncate_diff(&diff, 100); // 100 tokens ≈ 400 bytes
        assert!(out.contains("diff truncated"));
        assert!(out.len() < diff.len());
    }

    #[test]
    fn truncate_diff_preserves_leading_lines() {
        let mut diff = String::from("diff --git a/x b/x\nfile-header-line\n");
        diff.push_str(&"x".repeat(20_000));
        let out = truncate_diff(&diff, 100);
        assert!(out.starts_with("diff --git a/x b/x"));
        assert!(out.contains("file-header-line"));
        assert!(out.contains("diff truncated"));
    }
}
