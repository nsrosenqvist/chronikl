//! Per-run audit log.
//!
//! # Bounded Context: Run Audit
//!
//! Captures a structured, post-hoc record of *what actually happened*
//! during a chronikl run — every LLM call (Tier 1/2/3 and the prose
//! pass), every tool invocation in the agent loop (when Tier 3 lands),
//! token cost per call, prompt and response hashes, the final classified
//! commits, and the final rendered Markdown.
//!
//! The artifact is opt-in via the `--audit-log <PATH>` CLI flag or the
//! `CHRONIKL_AUDIT_LOG` env var. When unset, the entire pipeline runs
//! unchanged (the [`AuditSink`] is still constructed but never read).
//!
//! Veritrail-style: a frozen, diffable trail that lets future-you answer
//! "why did chronikl say X for v1.4.0?" without re-running the pipeline.

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::models::{Classified, TokenUsage};

/// Schema version of the audit document. Bump when the layout changes
/// in a way that breaks readers.
pub const AUDIT_SCHEMA_VERSION: u32 = 1;

/// Top-level audit document written to disk at the end of a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunAudit {
    pub schema: u32,
    pub run_id: String,
    pub started_at_unix_ms: u64,
    pub duration_ms: u64,
    pub chronikl_version: String,
    pub config: ConfigSnapshot,
    pub range: RangeSnapshot,
    pub voice_is_custom: bool,
    /// Canonical bundled-voice profile name (e.g. `"terse"`, `"prose"`).
    /// `None` when the user supplied a custom voice file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_name: Option<String>,
    /// One record per LLM call (Tier 1 batch, Tier 2 per-commit, Tier 3
    /// agent turn, prose pass) in invocation order.
    pub llm_calls: Vec<LlmCallAudit>,
    /// Aggregate token usage across every recorded call.
    pub tokens: TokenUsage,
    /// Final classification surface for each commit. Snapshotted after
    /// the ladder finishes so a reader can correlate calls → outcomes.
    pub final_classification: Classified,
    /// The Markdown produced by the run.
    pub rendered_markdown: String,
}

/// Static configuration snapshot — secrets are explicitly excluded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub agent_fallback: bool,
    pub max_diff_tokens: usize,
    pub confidence_threshold: f32,
    pub batch_size: usize,
    pub merge_style_override: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RangeSnapshot {
    pub from: Option<String>,
    pub to: String,
    pub commit_count: usize,
    pub detected_merge_style: String,
    /// True when the upper bound resolves to a prerelease semver tag.
    #[serde(default)]
    pub prerelease: bool,
    /// Prerelease label (e.g. `rc.1`) when `prerelease` is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_label: Option<String>,
    /// Detected version bump kind (`initial`, `patch`, `minor`, `major`,
    /// `unknown`).
    #[serde(default = "default_bump_string")]
    pub version_bump: String,
    /// Detected version scheme (`semver`, `calver`) when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_scheme: Option<String>,
}

fn default_bump_string() -> String {
    "unknown".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmCallAudit {
    /// Stage label: e.g. "tier-1-batch", "tier-2-per-commit",
    /// "tier-3-agent", "prose".
    pub label: String,
    pub provider: String,
    pub model: String,
    pub started_at_unix_ms: u64,
    pub duration_ms: u64,
    pub status: CallStatus,
    pub tokens: TokenUsage,
    /// SHA-256 of the (system_prompt + "\n\n" + user_prompt) sent to the
    /// model. Stable across runs — useful for diffing two audits of the
    /// same range to see what changed.
    pub prompt_hash: String,
    /// SHA-256 of the response text. Lets you spot when the same prompt
    /// produced a different answer (model drift, sampling).
    pub response_hash: Option<String>,
    /// Commit SHAs covered by this call (empty for the prose pass since
    /// it spans all of them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commit_shas: Vec<String>,
    /// Number of retries the provider performed before this terminal
    /// status. Zero on first-try success.
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub retries: usize,
    /// Agent-loop diagnostics, when this call drove a multi-turn
    /// tool-using loop (Tier 3). `None` for single-turn calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentDiagnostics>,
}

/// Per-run diagnostics for a multi-turn agent loop. Captures the loop's
/// shape (turns, termination cause) and every tool call the model made,
/// so a reader can answer "what did the model do here?".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDiagnostics {
    pub turns: u32,
    /// Set when the loop exited because the model invoked the configured
    /// terminal tool. `None` means the loop ended via turn budget or
    /// the model emitted text without calling the terminal tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminated_via_tool: Option<String>,
    /// True when the loop fired a self-repair correction (model emitted
    /// text instead of calling the terminal tool). Frequent self-repairs
    /// signal poor structured-output adherence on the chosen model.
    #[serde(default, skip_serializing_if = "is_false")]
    pub self_repair_attempted: bool,
    /// Tool calls in invocation order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub name: String,
    /// SHA-256 of the JSON args. Lets you diff two runs without storing
    /// possibly-large args verbatim.
    pub args_hash: String,
    /// Short human-readable arg summary (e.g. `src/lib.rs`, `pattern="fn main"`).
    pub args_summary: String,
    pub duration_ms: u64,
    /// SHA-256 of the result text. `None` on tool error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_hash: Option<String>,
    /// Short human-readable result summary (e.g. `1.2KB`, `3 results`).
    pub result_summary: String,
    /// True when the tool returned an error.
    #[serde(default, skip_serializing_if = "is_false")]
    pub failed: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CallStatus {
    Success,
    Failed { error: String },
}

fn is_zero_usize(v: &usize) -> bool {
    *v == 0
}

/// Compute SHA-256 hex of an input. Used for prompt and response hashes.
pub fn hash_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let digest = h.finalize();
    hex::encode(digest)
}

/// Wall-clock time as Unix epoch milliseconds.
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Thread-safe sink for audit records, shared between the orchestrator
/// and the provider via [`Arc`].
#[derive(Debug, Clone, Default)]
pub struct AuditSink {
    inner: Arc<Mutex<AuditState>>,
}

#[derive(Debug, Default)]
struct AuditState {
    enabled: bool,
    started_at_unix_ms: u64,
    started_instant: Option<std::time::Instant>,
    chronikl_version: String,
    voice_is_custom: bool,
    voice_name: Option<String>,
    config: Option<ConfigSnapshot>,
    range: Option<RangeSnapshot>,
    llm_calls: Vec<LlmCallAudit>,
}

impl AuditSink {
    /// New sink. The opt-in switch is set by [`AuditSink::enable`]; until
    /// then `record_call` is a no-op and the final document carries no
    /// records. Provider implementations always call `record_call`
    /// regardless — the sink decides whether to retain the data.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable recording. Idempotent.
    pub fn enable(&self) {
        let mut s = self.inner.lock().expect("audit lock poisoned");
        if !s.enabled {
            s.enabled = true;
            s.started_at_unix_ms = now_unix_ms();
            s.started_instant = Some(std::time::Instant::now());
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.lock().expect("audit lock poisoned").enabled
    }

    pub fn set_chronikl_version(&self, v: String) {
        self.inner
            .lock()
            .expect("audit lock poisoned")
            .chronikl_version = v;
    }

    pub fn set_voice_is_custom(&self, b: bool) {
        self.inner
            .lock()
            .expect("audit lock poisoned")
            .voice_is_custom = b;
    }

    /// Record the canonical bundled-voice profile name (e.g. `"terse"`).
    /// Pass `None` for custom (file-based) voices.
    pub fn set_voice_name(&self, name: Option<String>) {
        self.inner.lock().expect("audit lock poisoned").voice_name = name;
    }

    pub fn set_config(&self, c: ConfigSnapshot) {
        self.inner.lock().expect("audit lock poisoned").config = Some(c);
    }

    pub fn set_range(&self, r: RangeSnapshot) {
        self.inner.lock().expect("audit lock poisoned").range = Some(r);
    }

    /// Lightweight totals snapshot for the run summary. Returns
    /// `(call_count, aggregated_tokens)`. Only counts records present
    /// in the buffer — disabled sinks return `(0, default)`.
    pub fn totals(&self) -> (usize, TokenUsage) {
        let s = self.inner.lock().expect("audit lock poisoned");
        let mut total = TokenUsage::default();
        for c in &s.llm_calls {
            total += c.tokens;
        }
        (s.llm_calls.len(), total)
    }

    /// Append an LLM call record. Always-on so providers don't have to
    /// branch on enablement; the sink discards records when disabled.
    pub fn record_call(&self, call: LlmCallAudit) {
        let mut s = self.inner.lock().expect("audit lock poisoned");
        if s.enabled {
            s.llm_calls.push(call);
        }
    }

    /// Snapshot the run as a [`RunAudit`] given the final classification
    /// and rendered Markdown.
    pub fn finalize(
        &self,
        final_classification: Classified,
        rendered_markdown: String,
    ) -> RunAudit {
        let s = self.inner.lock().expect("audit lock poisoned");
        let duration_ms = s
            .started_instant
            .as_ref()
            .map(|i| i.elapsed().as_millis() as u64)
            .unwrap_or(0);
        let mut total = TokenUsage::default();
        for c in &s.llm_calls {
            total += c.tokens;
        }
        RunAudit {
            schema: AUDIT_SCHEMA_VERSION,
            run_id: uuid::Uuid::new_v4().to_string(),
            started_at_unix_ms: s.started_at_unix_ms,
            duration_ms,
            chronikl_version: s.chronikl_version.clone(),
            config: s.config.clone().unwrap_or_else(empty_config_snapshot),
            range: s.range.clone().unwrap_or_else(empty_range_snapshot),
            voice_is_custom: s.voice_is_custom,
            voice_name: s.voice_name.clone(),
            llm_calls: s.llm_calls.clone(),
            tokens: total,
            final_classification,
            rendered_markdown,
        }
    }
}

fn empty_config_snapshot() -> ConfigSnapshot {
    ConfigSnapshot {
        provider: None,
        model: None,
        agent_fallback: false,
        max_diff_tokens: 0,
        confidence_threshold: 0.0,
        batch_size: 0,
        merge_style_override: None,
    }
}

fn empty_range_snapshot() -> RangeSnapshot {
    RangeSnapshot {
        from: None,
        to: String::new(),
        commit_count: 0,
        detected_merge_style: String::new(),
        prerelease: false,
        release_label: None,
        version_bump: "unknown".into(),
        version_scheme: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_hex_is_stable_and_64_chars() {
        let h = hash_hex("hello world");
        assert_eq!(h.len(), 64);
        assert_eq!(h, hash_hex("hello world"));
        assert_ne!(h, hash_hex("hello world!"));
    }

    #[test]
    fn disabled_sink_drops_calls() {
        let sink = AuditSink::new();
        sink.record_call(sample_call());
        let audit = sink.finalize(Classified::default(), String::new());
        assert!(audit.llm_calls.is_empty());
    }

    #[test]
    fn enabled_sink_records_calls() {
        let sink = AuditSink::new();
        sink.enable();
        sink.record_call(sample_call());
        sink.record_call(sample_call());
        let audit = sink.finalize(Classified::default(), String::new());
        assert_eq!(audit.llm_calls.len(), 2);
    }

    #[test]
    fn finalize_aggregates_tokens() {
        let sink = AuditSink::new();
        sink.enable();
        let mut a = sample_call();
        a.tokens = TokenUsage::new(10, 5, 0);
        sink.record_call(a);
        let mut b = sample_call();
        b.tokens = TokenUsage::new(20, 7, 3);
        sink.record_call(b);
        let audit = sink.finalize(Classified::default(), String::new());
        assert_eq!(audit.tokens.input_tokens, 30);
        assert_eq!(audit.tokens.output_tokens, 12);
        assert_eq!(audit.tokens.cached_input_tokens, 3);
    }

    #[test]
    fn run_audit_round_trips_through_serde() {
        let sink = AuditSink::new();
        sink.enable();
        sink.record_call(sample_call());
        let audit = sink.finalize(Classified::default(), "# Notes".to_string());
        let json = serde_json::to_string(&audit).unwrap();
        let back: RunAudit = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema, AUDIT_SCHEMA_VERSION);
        assert_eq!(back.rendered_markdown, "# Notes");
        assert_eq!(back.llm_calls.len(), 1);
    }

    fn sample_call() -> LlmCallAudit {
        LlmCallAudit {
            label: "tier-1-batch".into(),
            provider: "anthropic".into(),
            model: "claude-haiku-4-5".into(),
            started_at_unix_ms: 0,
            duration_ms: 100,
            status: CallStatus::Success,
            tokens: TokenUsage::default(),
            prompt_hash: hash_hex("system\n\nuser"),
            response_hash: Some(hash_hex("response")),
            commit_shas: vec!["abc".into()],
            retries: 0,
            agent: None,
        }
    }
}
