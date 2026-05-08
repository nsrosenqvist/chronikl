//! Tier 3 — agentic LLM classification with read tools.
//!
//! Last-resort fallback for commits Tier 0/1/2 still couldn't
//! confidently classify (`confidence < threshold`). Runs an agent loop
//! per commit: the model gets `read_file`, `list_directory`, and
//! `search_text` tools plus the `submit_classification` terminal tool.
//! It explores the repo to figure out what the commit really did, then
//! exits via the terminal tool with a [`Tier3Verdict`].
//!
//! The audit log captures the loop's full diagnostics (turns, tool
//! calls, termination reason) so a reader can answer "what did the
//! model do here?" — the veritrail-style trail the project requires.

use std::path::Path;
use std::sync::Arc;

use futures::stream::{StreamExt, TryStreamExt};
use rig::tool::ToolDyn;

use crate::audit::{AuditSink, hash_hex, now_unix_ms};
use crate::git;
use crate::ladder::tier2;
use crate::models::{Classification, ClassificationSource, Classified, Section};
use crate::providers::ProviderError;
use crate::providers::rig::RigProvider;
use crate::tools::{
    GitShowTool, ListDirectoryTool, ReadFileTool, SUBMIT_CLASSIFICATION_TOOL_NAME, SearchTextTool,
    SubmitClassificationTool,
};

use crate::constants::{LLM_CONCURRENCY, TIER3_MAX_OUTPUT_TOKENS};
/// Default per-commit cap on agent-loop turns. Each turn = one model
/// call. The model can typically figure out a commit in 3-6 turns.
const DEFAULT_MAX_TURNS: u32 = 10;

/// Run Tier 3 over a [`Classified`] in-place. Returns the number of
/// commits whose classification was updated.
///
/// Per-commit agent loops run concurrently (bounded by
/// [`LLM_CONCURRENCY`]) — each loop's tool registry includes a
/// dedicated `submit_classification` slot, so loops don't share
/// captured state.
#[allow(clippy::too_many_arguments)]
pub async fn classify(
    classified: &mut Classified,
    repo: &Path,
    provider: &RigProvider,
    audit: &AuditSink,
    max_diff_tokens: usize,
    confidence_threshold: f32,
    max_turns: u32,
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

    // Pre-fetch diffs sequentially (git is local + cheap) so the
    // concurrent agent loops don't have to share `repo` mutably.
    struct Job {
        global_index: usize,
        sha: String,
        short_sha: String,
        user_prompt: String,
    }
    let mut jobs: Vec<Job> = Vec::with_capacity(pending.len());
    for global_index in pending {
        let entry = &classified.0[global_index];
        let diff_raw = git::commit_diff(repo, &entry.commit.sha)
            .await
            .map_err(|e| ProviderError::Api(format!("git show failed: {e}")))?;
        let diff = tier2::truncate_diff(&diff_raw, max_diff_tokens);
        jobs.push(Job {
            global_index,
            sha: entry.commit.sha.clone(),
            short_sha: entry.commit.short_sha.clone(),
            user_prompt: build_prompt(entry, &diff),
        });
    }

    type AgentVerdict = (usize, String, Option<crate::tools::Tier3Verdict>);
    let results: Vec<AgentVerdict> = futures::stream::iter(jobs.iter().map(|job| {
        async move {
            // Build tool registry afresh per commit so the terminal
            // tool's captured slot is isolated per call.
            let (submit, captured) = SubmitClassificationTool::new();
            let tools: Vec<Arc<dyn ToolDyn>> = vec![
                Arc::new(submit),
                Arc::new(ReadFileTool::new(repo.to_path_buf())),
                Arc::new(ListDirectoryTool::new(repo.to_path_buf())),
                Arc::new(SearchTextTool::new(repo.to_path_buf())),
                Arc::new(GitShowTool::new(repo.to_path_buf())),
            ];

            let prompt_hash = hash_hex(&format!("{system_prompt}\n\n{}", job.user_prompt));
            let started_at = now_unix_ms();
            let started_instant = std::time::Instant::now();
            let outcome = provider
                .run_agent(
                    system_prompt,
                    &job.user_prompt,
                    tools,
                    TIER3_MAX_OUTPUT_TOKENS,
                    max_turns.max(1),
                    Some(SUBMIT_CLASSIFICATION_TOOL_NAME),
                    Some(crate::prompts::TIER3_SELF_REPAIR),
                )
                .await;
            let duration_ms = started_instant.elapsed().as_millis() as u64;

            provider.record_agent_audit(
                audit,
                "tier-3-agent",
                vec![job.sha.clone()],
                prompt_hash,
                &outcome,
                started_at,
                duration_ms,
            );

            // Surface the loop error, otherwise drain the captured slot.
            outcome?;
            let captured_value = captured.lock().map(|g| g.clone()).unwrap_or(None);
            Ok::<_, ProviderError>((job.global_index, job.short_sha.clone(), captured_value))
        }
    }))
    .buffer_unordered(LLM_CONCURRENCY)
    .try_collect()
    .await?;

    let mut updated = 0usize;
    for (global_index, short_sha, verdict) in results {
        let Some(verdict) = verdict else {
            // Loop ended without the model calling submit_classification.
            // Skip this commit (leave its existing classification).
            eprintln!("warn: Tier 3 model did not call submit_classification for {short_sha}");
            continue;
        };
        let section = if verdict.breaking {
            Section::Breaking
        } else {
            Section::parse_lenient(&verdict.section).unwrap_or(Section::Other)
        };
        classified.0[global_index].classification = Classification {
            section,
            summary: verdict.summary.trim().to_string(),
            source: ClassificationSource::Agentic,
            confidence: verdict.confidence.clamp(0.0, 1.0),
        };
        updated += 1;
    }
    Ok(updated)
}

/// Default Tier 3 system prompt. Loaded from `src/prompts/tier3_system.md`.
pub fn default_system_prompt() -> &'static str {
    crate::prompts::TIER3_SYSTEM
}

/// Public for the `chronikl debug prompts` subcommand. Builds the
/// initial user prompt Tier 3's agent loop would send (subsequent
/// turns depend on tool calls and aren't deterministic).
pub fn debug_prompt(entry: &crate::models::ClassifiedCommit, diff: &str) -> String {
    build_prompt(entry, diff)
}

fn build_prompt(entry: &crate::models::ClassifiedCommit, diff: &str) -> String {
    let mut out = format!(
        "Classify the following commit ({}).\n\n",
        entry.commit.short_sha
    );
    out.push_str(&crate::prompts::commit_context_prompt(entry, diff));
    out.push_str(
        "\n\nExplore the repo if it helps you classify with confidence, then call \
         `submit_classification` with your final verdict.",
    );
    out
}

/// Exposed for callers (main.rs) that need the default turn cap.
pub const fn default_max_turns() -> u32 {
    DEFAULT_MAX_TURNS
}
