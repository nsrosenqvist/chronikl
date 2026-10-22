//! Tier 1 — batched LLM classification (no diff).
//!
//! For commits Tier 0 couldn't confidently place (confidence < 1.0) we
//! batch them into groups and ask a cheap model to classify each one.
//! Input per commit: subject + body summary + file paths. Output:
//! a structured list with section + summary + confidence per commit.
//!
//! Why batch: amortizes the system prompt across a group, so 300
//! commits cost ~6 calls in the worst case rather than 300.
//!
//! Cost control: we *only* send commits that Tier 0 left unclassified
//! or low-confidence. Conventional / files-only / squash commits stay
//! at confidence 1.0 from Tier 0 and skip Tier 1 entirely.

use std::collections::HashMap;

use futures::stream::{StreamExt, TryStreamExt};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::audit::AuditSink;
use crate::models::{Classification, ClassificationSource, Classified, ClassifiedCommit, Section};
use crate::providers::response::parse_with_fallbacks;
use crate::providers::{CompletionRequest, NotesProvider, ProviderError};

/// Default cap on how much per-commit content we send to the model in
/// the batch prompt. Keeps a single batch from blowing up when a
/// repo has unusually verbose commit bodies or hundreds of files.
const BODY_PREVIEW_CHARS: usize = 600;
const FILES_PREVIEW: usize = 12;

use crate::constants::{LLM_CONCURRENCY, TIER1_MAX_OUTPUT_TOKENS};

/// Schema sent to the model for each batch entry it returns.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BatchVerdict {
    /// Index of the commit in the batch (0-based).
    pub index: usize,
    /// Section name. Lowercase kebab-case to match [`Section`]'s serde rename.
    pub section: String,
    /// One-line summary suitable for the rendered bullet.
    pub summary: String,
    /// Model's confidence, 0.0..=1.0.
    pub confidence: f32,
}

/// Wrapper for the response shape so providers that prefer object roots
/// (rather than top-level arrays) still parse cleanly.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BatchResponse {
    pub verdicts: Vec<BatchVerdict>,
}

/// Run Tier 1 over the given classification, in-place. Commits with
/// confidence >= `min_confidence_to_skip` are passed through; the rest
/// are sent to the model in batches of `batch_size`.
///
/// `system_prompt` is provided by the caller (see [`default_system_prompt`])
/// so tests can pin it.
pub async fn classify(
    classified: &mut Classified,
    provider: &dyn NotesProvider,
    _audit: &AuditSink,
    batch_size: usize,
    min_confidence_to_skip: f32,
    system_prompt: &str,
) -> Result<usize, ProviderError> {
    // Find commits that need Tier 1.
    let pending: Vec<usize> = classified
        .0
        .iter()
        .enumerate()
        .filter(|(_, c)| c.classification.confidence < min_confidence_to_skip)
        .map(|(i, _)| i)
        .collect();

    if pending.is_empty() {
        return Ok(0);
    }

    let mut updated = apply_verdicts(
        classified,
        run_round(classified, &pending, provider, batch_size, system_prompt).await?,
    );
    let first_round: std::collections::HashSet<usize> = updated.iter().copied().collect();

    // Models occasionally return verdicts for only part of a batch (seen
    // with Claude 5.5 at its default temperature). Ask once more for the
    // commits that got none, in their own smaller batch.
    let missing: Vec<usize> = pending
        .iter()
        .copied()
        .filter(|i| !first_round.contains(i))
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "warn: Tier 1 returned no verdict for {} of {} commit(s); asking again for those",
            missing.len(),
            pending.len()
        );
        updated.extend(apply_verdicts(
            classified,
            run_round(classified, &missing, provider, batch_size, system_prompt).await?,
        ));
    }
    Ok(updated.len())
}

/// Verdicts for one batch: the batch's global commit indices and the
/// model's verdicts keyed by local index.
type BatchResult = (Vec<usize>, HashMap<usize, BatchVerdict>);

/// Send `indices` to the model in batches of `batch_size`, concurrently
/// (bounded by [`LLM_CONCURRENCY`]). Short-circuits on the first error.
async fn run_round(
    classified: &Classified,
    indices: &[usize],
    provider: &dyn NotesProvider,
    batch_size: usize,
    system_prompt: &str,
) -> Result<Vec<BatchResult>, ProviderError> {
    let schema =
        serde_json::to_value(schema_for!(BatchResponse)).unwrap_or(serde_json::Value::Null);

    // Build per-batch jobs. Each job owns the data it needs (prompt,
    // label, shas) so the futures don't borrow `classified`.
    struct BatchJob {
        chunk: Vec<usize>,
        user_prompt: String,
        label: String,
        shas: Vec<String>,
    }
    let jobs: Vec<BatchJob> = indices
        .chunks(batch_size.max(1))
        .map(|chunk| {
            let entries: Vec<&ClassifiedCommit> = chunk.iter().map(|&i| &classified.0[i]).collect();
            BatchJob {
                chunk: chunk.to_vec(),
                user_prompt: build_batch_prompt(&entries),
                label: format!("tier-1-batch-{}-commits", entries.len()),
                shas: entries.iter().map(|c| c.commit.sha.clone()).collect(),
            }
        })
        .collect();

    futures::stream::iter(jobs.iter().map(|job| {
        let schema = schema.clone();
        async move {
            let request = CompletionRequest {
                system_prompt,
                user_prompt: &job.user_prompt,
                schema,
                max_tokens: TIER1_MAX_OUTPUT_TOKENS,
                label: &job.label,
                commit_shas: &job.shas,
            };
            let response = provider.complete(request).await?;
            let verdicts = parse_batch_response(&response.text)?;
            let by_index: HashMap<usize, BatchVerdict> =
                verdicts.into_iter().map(|v| (v.index, v)).collect();
            Ok::<_, ProviderError>((job.chunk.clone(), by_index))
        }
    }))
    .buffer_unordered(LLM_CONCURRENCY)
    .try_collect()
    .await
}

/// Write verdicts into `classified`; returns the indices that got one.
fn apply_verdicts(classified: &mut Classified, results: Vec<BatchResult>) -> Vec<usize> {
    let mut updated = Vec::new();
    for (chunk, by_index) in results {
        for (local_index, global_index) in chunk.into_iter().enumerate() {
            if let Some(verdict) = by_index.get(&local_index) {
                let mut section =
                    Section::parse_lenient(&verdict.section).unwrap_or(Section::Other);
                let mut confidence = verdict.confidence.clamp(0.0, 1.0);
                if let Some((corrected, cap)) = crate::ladder::files::correct_section(
                    section,
                    &classified.0[global_index].commit.files,
                ) {
                    section = corrected;
                    confidence = confidence.min(cap);
                }
                classified.0[global_index].classification = Classification {
                    section,
                    summary: verdict.summary.trim().to_string(),
                    source: ClassificationSource::BatchedLlm,
                    confidence,
                };
                updated.push(global_index);
            }
        }
    }
    updated
}

/// Default Tier 1 system prompt. Loaded from `src/prompts/tier1_system.md`.
pub fn default_system_prompt() -> &'static str {
    crate::prompts::TIER1_SYSTEM
}

/// Public for the `chronikl debug prompts` subcommand. Builds the
/// exact user prompt Tier 1 would send for a given batch of commits.
pub fn debug_batch_prompt(commits: &[&ClassifiedCommit]) -> String {
    build_batch_prompt(commits)
}

fn build_batch_prompt(commits: &[&ClassifiedCommit]) -> String {
    use crate::prompts::fence;
    let mut out = String::new();
    out.push_str("Classify these commits.\n\n");
    for (i, c) in commits.iter().enumerate() {
        out.push_str(&format!("--- commit {i} ---\n"));
        // All attacker-controllable fields are XML-fenced so the
        // tier-1 system prompt can refer to them as untrusted data
        // zones. See `crate::prompts::fence` for the sanitization.
        out.push_str(&fence("commit_subject", &c.commit.subject));
        out.push('\n');
        if !c.commit.body.trim().is_empty() {
            let body = truncate(&c.commit.body, BODY_PREVIEW_CHARS);
            out.push_str(&fence("commit_body", &body));
            out.push('\n');
        }
        // PR enrichment data — when present, the PR title is usually a
        // better signal than the raw commit subject (especially in
        // squash-merge repos), and labels can confirm the section
        // (e.g. `breaking-change`, `enhancement`).
        if let Some(pr) = &c.pr {
            out.push_str(&fence("pr_title", &pr.title));
            out.push('\n');
            if !pr.body.trim().is_empty() {
                let body = truncate(&pr.body, BODY_PREVIEW_CHARS);
                out.push_str(&fence("pr_body", &body));
                out.push('\n');
            }
            if !pr.labels.is_empty() {
                out.push_str(&fence("pr_labels", &pr.labels.join(", ")));
                out.push('\n');
            }
        }
        if !c.commit.files.is_empty() {
            // Product files first: a feature with many test files
            // shouldn't read as a tests commit in a truncated list.
            let ordered = crate::ladder::files::product_first(&c.commit.files);
            let files: Vec<&str> = ordered.iter().take(FILES_PREVIEW).copied().collect();
            out.push_str(&format!(
                "files ({}): {}\n",
                crate::ladder::files::summary(&c.commit.files),
                files.join(", ")
            ));
            if c.commit.files.len() > FILES_PREVIEW {
                out.push_str(&format!(
                    "  (+{} more files)\n",
                    c.commit.files.len() - FILES_PREVIEW
                ));
            }
        }
        out.push('\n');
    }
    out.push_str(
        "Return JSON with shape {\"verdicts\": [{\"index\": 0, \"section\": \"...\", \
         \"summary\": \"...\", \"confidence\": 0.0}]}.\n",
    );
    out
}

fn truncate(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let mut out: String = s.chars().take(cap).collect();
    out.push_str(" …");
    out
}

fn parse_batch_response(text: &str) -> Result<Vec<BatchVerdict>, ProviderError> {
    // Try the wrapped shape first, then a top-level array.
    if let Ok(wrapped) = parse_with_fallbacks::<BatchResponse>(text) {
        return Ok(wrapped.verdicts);
    }
    parse_with_fallbacks::<Vec<BatchVerdict>>(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Classification, ClassificationSource, Commit, Section};
    use crate::providers::mock::MockProvider;

    fn commit(sha: &str, subject: &str, body: &str, files: &[&str]) -> ClassifiedCommit {
        ClassifiedCommit {
            commit: Commit {
                sha: sha.to_string(),
                short_sha: sha[..7.min(sha.len())].to_string(),
                author_name: "ada".into(),
                author_email: "ada@x".into(),
                author_date: "2026-01-01T00:00:00+00:00".into(),
                parents: vec!["p".into()],
                subject: subject.into(),
                body: body.into(),
                files: files.iter().map(|s| s.to_string()).collect(),
                pr_id: None,
                conventional: None,
                breaking: false,
            },
            pr: None,
            classification: Classification {
                section: Section::Other,
                summary: subject.into(),
                source: ClassificationSource::Default,
                confidence: 0.5,
            },
        }
    }

    #[tokio::test]
    async fn batches_only_low_confidence_commits() {
        let mut classified = Classified(vec![
            commit("a", "wip", "", &["src/a.rs"]),
            ClassifiedCommit {
                classification: Classification {
                    section: Section::Features,
                    summary: "add login".into(),
                    source: ClassificationSource::Conventional,
                    confidence: 1.0,
                },
                ..commit("b", "feat: add login", "", &["src/b.rs"])
            },
        ]);

        let resp = serde_json::json!({
            "verdicts": [
                {"index": 0, "section": "other", "summary": "wip", "confidence": 0.2}
            ]
        })
        .to_string();
        let provider = MockProvider::returning(resp);
        let audit = AuditSink::new();

        let updated = classify(
            &mut classified,
            &provider,
            &audit,
            10,
            1.0,
            default_system_prompt(),
        )
        .await
        .unwrap();

        assert_eq!(updated, 1, "should reclassify only the low-confidence one");
        assert_eq!(provider.call_count(), 1, "single batch call expected");
        // The pre-classified Features commit was untouched.
        assert_eq!(classified.0[1].classification.section, Section::Features);
        assert_eq!(classified.0[1].classification.confidence, 1.0);
        // The low-confidence commit was updated by the LLM.
        assert!(matches!(
            classified.0[0].classification.source,
            ClassificationSource::BatchedLlm
        ));
    }

    #[tokio::test]
    async fn splits_into_multiple_batches() {
        let commits: Vec<ClassifiedCommit> = (0..7)
            .map(|i| commit(&format!("sha{i}"), &format!("subject {i}"), "", &["x"]))
            .collect();
        let mut classified = Classified(commits);

        let make_batch_response = |start: usize, end: usize| -> String {
            let verdicts: Vec<_> = (0..(end - start))
                .map(|i| {
                    serde_json::json!({
                        "index": i,
                        "section": "other",
                        "summary": format!("subject {}", start + i),
                        "confidence": 0.4,
                    })
                })
                .collect();
            serde_json::json!({"verdicts": verdicts}).to_string()
        };

        let provider = MockProvider::returning_sequence(vec![
            make_batch_response(0, 3),
            make_batch_response(3, 6),
            make_batch_response(6, 7),
        ]);
        let audit = AuditSink::new();

        let updated = classify(
            &mut classified,
            &provider,
            &audit,
            3,
            1.0,
            default_system_prompt(),
        )
        .await
        .unwrap();
        assert_eq!(updated, 7);
        assert_eq!(provider.call_count(), 3);
    }

    #[tokio::test]
    async fn asks_again_for_commits_the_model_skipped() {
        let commits: Vec<ClassifiedCommit> = (0..4)
            .map(|i| commit(&format!("sha{i}"), &format!("subject {i}"), "", &[]))
            .collect();
        let mut classified = Classified(commits);
        // First answer covers only commit 0; the retry batch holds the
        // other three, re-indexed from 0.
        let first = serde_json::json!({"verdicts": [
            {"index": 0, "section": "features", "summary": "s0", "confidence": 0.9}
        ]})
        .to_string();
        let retry = serde_json::json!({"verdicts": [
            {"index": 0, "section": "bug-fixes", "summary": "s1", "confidence": 0.9},
            {"index": 1, "section": "tests", "summary": "s2", "confidence": 0.9},
            {"index": 2, "section": "docs", "summary": "s3", "confidence": 0.9}
        ]})
        .to_string();
        let provider = MockProvider::returning_sequence(vec![first, retry]);
        let audit = AuditSink::new();

        let updated = classify(
            &mut classified,
            &provider,
            &audit,
            50,
            1.0,
            default_system_prompt(),
        )
        .await
        .unwrap();
        assert_eq!(updated, 4);
        assert_eq!(provider.call_count(), 2);
        let retry_prompt = &provider.captured_calls()[1].user_prompt;
        assert!(!retry_prompt.contains("subject 0"));
        assert!(retry_prompt.contains("subject 3"));
        let sections: Vec<Section> = classified
            .iter()
            .map(|c| c.classification.section)
            .collect();
        assert_eq!(
            sections,
            vec![
                Section::Features,
                Section::BugFixes,
                Section::Tests,
                Section::Documentation
            ]
        );
    }

    #[tokio::test]
    async fn pure_other_section_choice_maps_to_section_enum() {
        let mut classified = Classified(vec![commit("a", "weird", "", &["x"])]);
        let resp = serde_json::json!({
            "verdicts": [
                {"index": 0, "section": "bug-fixes", "summary": "Fix weird", "confidence": 0.7}
            ]
        })
        .to_string();
        let provider = MockProvider::returning(resp);
        let audit = AuditSink::new();
        classify(
            &mut classified,
            &provider,
            &audit,
            10,
            1.0,
            default_system_prompt(),
        )
        .await
        .unwrap();
        assert_eq!(classified.0[0].classification.section, Section::BugFixes);
        assert_eq!(classified.0[0].classification.summary, "Fix weird");
    }

    #[tokio::test]
    async fn unknown_section_falls_back_to_other() {
        let mut classified = Classified(vec![commit("a", "weird", "", &["x"])]);
        let resp = serde_json::json!({
            "verdicts": [
                {"index": 0, "section": "spaghetti", "summary": "x", "confidence": 0.5}
            ]
        })
        .to_string();
        let provider = MockProvider::returning(resp);
        let audit = AuditSink::new();
        classify(
            &mut classified,
            &provider,
            &audit,
            10,
            1.0,
            default_system_prompt(),
        )
        .await
        .unwrap();
        assert_eq!(classified.0[0].classification.section, Section::Other);
    }

    #[tokio::test]
    async fn provider_error_propagates() {
        let mut classified = Classified(vec![commit("a", "weird", "", &["x"])]);
        let provider = MockProvider::failing("boom");
        let audit = AuditSink::new();
        let err = classify(
            &mut classified,
            &provider,
            &audit,
            10,
            1.0,
            default_system_prompt(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ProviderError::Api(_)));
    }

    #[tokio::test]
    async fn parses_top_level_array_response() {
        let mut classified = Classified(vec![commit("a", "weird", "", &["x"])]);
        // Top-level array (no wrapping `{"verdicts": …}`) — providers
        // sometimes drop the wrapper despite the schema.
        let resp = serde_json::json!([
            {"index": 0, "section": "other", "summary": "weird", "confidence": 0.4}
        ])
        .to_string();
        let provider = MockProvider::returning(resp);
        let audit = AuditSink::new();
        classify(
            &mut classified,
            &provider,
            &audit,
            10,
            1.0,
            default_system_prompt(),
        )
        .await
        .unwrap();
        assert!(matches!(
            classified.0[0].classification.source,
            ClassificationSource::BatchedLlm
        ));
    }

    #[test]
    fn truncate_keeps_short_strings() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_caps_long_strings() {
        let out = truncate("a".repeat(20).as_str(), 5);
        assert!(out.starts_with("aaaaa"));
        assert!(out.ends_with('…'));
    }

    #[tokio::test]
    async fn tests_verdict_on_a_product_change_is_corrected() {
        // Montiva 672cffa2 "Landing FAQ: an iPhone app is planned": the
        // landing page's FAQ text in five locales plus the roadmap. Five
        // of six models filed it under documentation.
        let mut classified = Classified(vec![commit(
            "672cffa2",
            "Landing FAQ: an iPhone app is planned; until then the browser",
            "",
            &[
                "apps/web/src/presentation/i18n/locales/en-US/landing.json",
                "apps/web/src/presentation/i18n/locales/sv/landing.json",
                "docs/roadmap.md",
            ],
        )]);
        let provider = MockProvider::returning(
            serde_json::json!({"verdicts": [
                {"index": 0, "section": "documentation", "summary": "Update the FAQ", "confidence": 0.9}
            ]})
            .to_string(),
        );
        classify(
            &mut classified,
            &provider,
            &AuditSink::new(),
            50,
            1.0,
            default_system_prompt(),
        )
        .await
        .unwrap();
        let c = &classified.0[0].classification;
        assert_eq!(c.section, Section::Other);
        assert!(c.confidence <= 0.5);
    }

    #[test]
    fn batch_prompt_lists_product_files_first_with_a_summary() {
        // Montiva 8bf4ff12 "Map: Tools with Record a ride": the e2e specs
        // sort first alphabetically and used to fill the preview.
        let mut files: Vec<String> = (0..12)
            .map(|i| format!("apps/web/e2e/spec{i:02}.spec.ts"))
            .collect();
        files.push("apps/web/src/main.ts".into());
        files.push("apps/web/src/presentation/components/map/ToolsMenu.vue".into());
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let c = commit(
            "8bf4ff12",
            "Map: Tools with Record a ride, and the sharing pill in the top row",
            "",
            &refs,
        );
        let prompt = build_batch_prompt(&[&c]);
        assert!(prompt.contains("files (2 product, 12 test, 0 doc): apps/web/src/main.ts, "));
        assert!(prompt.contains("ToolsMenu.vue"));
    }

    #[test]
    fn system_prompt_carries_the_section_rules() {
        let p = default_system_prompt();
        assert!(p.contains("A\n  feature or fix that also adds tests is a feature or fix."));
        assert!(p.contains("Copy the product shows its users"));
    }
}
