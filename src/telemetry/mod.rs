//! Anonymous usage telemetry — privacy-respecting heartbeat.
//!
//! # Bounded Context: Telemetry
//!
//! Owns the heartbeat payload construction and the fire-and-forget HTTP
//! POST that runs at the end of each generate run. Carries only
//! aggregate counters — never commit text, classifications, prose,
//! or API keys.
//!
//! Disabled when:
//! - `--no-telemetry` is set on the CLI
//! - `CHRONIKL_TELEMETRY=false` (or any falsy value) is in the env
//! - `[telemetry] enabled = false` in `.chronikl.toml`
//!
//! All failures are silent: a failed heartbeat must never affect the
//! release-notes outcome.

use serde::Serialize;

use crate::ci;
use crate::constants::{ENV_DEBUG, FULL_VERSION, TELEMETRY_URL};

/// Anonymous payload posted at the end of each run.
///
/// Aggregates only — anyone reading the JSON should not be able to
/// reconstruct what the user was working on. Specifically excluded:
/// commit messages, file paths, voice content, prose output, classified
/// summaries, model output, prompts, or any user-supplied text.
#[derive(Debug, Clone, Serialize)]
pub struct HeartbeatPayload {
    /// One-shot UUID v4 for log correlation. Not persisted across runs.
    pub run_id: String,
    /// Total commits in the resolved range.
    pub commit_count: usize,
    /// Per-tier counts (how many commits each tier touched). Lets us
    /// see, in aggregate, how often Tier 2 / Tier 3 fire.
    pub tier_counts: TierCounts,
    /// Provider name (e.g. `"anthropic"`). `None` if no LLM ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Model id (e.g. `"claude-sonnet-4-6"`). `None` if no LLM ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Detected merge style — informative for sectioning quality work.
    pub merge_style: String,
    /// True when the target ref resolves to a prerelease semver tag
    /// (e.g. `v1.0.0-rc.1`).
    pub prerelease: bool,
    /// `"initial"` / `"patch"` / `"minor"` / `"major"` / `"unknown"`.
    /// Lets us see (in aggregate) what bump distribution chronikl runs
    /// see in the wild.
    pub version_bump: String,
    /// `"semver"` / `"calver"` / `null`. Aggregate distribution of
    /// recognised version schemes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_scheme: Option<String>,
    /// True when the user supplied their own voice (vs. the bundled
    /// default). The voice content itself is never sent.
    pub custom_voice: bool,
    /// True when Tier 3 agentic ran on at least one commit.
    pub agent_used: bool,
    /// True when PR enrichment was active (octocrab/GitHub).
    pub pr_enrichment: bool,
    /// True when an audit log was requested.
    pub audit_log: bool,
    /// True when a commercial license was active. Placeholder until the
    /// license module lands; currently always false.
    pub licensed: bool,
    /// Heuristic CI detection.
    pub is_ci: bool,
    /// Did the run finish without bubbling an error?
    pub success: bool,
    /// Build version string.
    pub version: &'static str,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TierCounts {
    pub tier0: usize,
    pub tier1: usize,
    pub tier2: usize,
    pub tier3: usize,
    /// Commits whose final classification came from a cache hit
    /// (skipped Tier 1+).
    pub cache_hits: usize,
}

impl HeartbeatPayload {
    /// Construct from the run's aggregate counters. Does not perform any
    /// I/O — call [`send_heartbeat`] separately to actually post.
    #[allow(clippy::too_many_arguments)]
    pub fn from_release(
        commit_count: usize,
        tier_counts: TierCounts,
        provider: Option<String>,
        model: Option<String>,
        merge_style: impl Into<String>,
        prerelease: bool,
        version_bump: impl Into<String>,
        version_scheme: Option<String>,
        custom_voice: bool,
        agent_used: bool,
        pr_enrichment: bool,
        audit_log: bool,
        licensed: bool,
        success: bool,
    ) -> Self {
        Self {
            run_id: uuid::Uuid::new_v4().to_string(),
            commit_count,
            tier_counts,
            provider,
            model,
            merge_style: merge_style.into(),
            prerelease,
            version_bump: version_bump.into(),
            version_scheme,
            custom_voice,
            agent_used,
            pr_enrichment,
            audit_log,
            licensed,
            is_ci: ci::is_ci(),
            success,
            version: FULL_VERSION,
        }
    }
}

/// True when `CHRONIKL_DEBUG` is set to a truthy value. Enables the
/// debug-print path in `send_heartbeat` so users can see exactly what
/// would be POSTed.
pub fn is_debug() -> bool {
    matches!(
        std::env::var(ENV_DEBUG).as_deref(),
        Ok("1" | "true" | "yes" | "on")
    )
}

/// Spawn a fire-and-forget heartbeat POST.
///
/// Returns the [`tokio::task::JoinHandle`] so callers that care about
/// completion (e.g. tests, or a binary that wants to wait briefly
/// before exiting) can await it. Letting the handle drop is safe — the
/// task will continue in the background until the runtime shuts down.
pub fn send_heartbeat(payload: HeartbeatPayload) -> tokio::task::JoinHandle<()> {
    if is_debug() {
        tokio::spawn(async move {
            debug_post(&payload).await;
        })
    } else {
        tokio::spawn(async move {
            let _ = post(&payload).await;
        })
    }
}

async fn post(payload: &HeartbeatPayload) -> Result<(), Box<dyn std::error::Error>> {
    let client = crate::http::build_client()?;
    client.post(TELEMETRY_URL).json(payload).send().await?;
    Ok(())
}

async fn debug_post(payload: &HeartbeatPayload) {
    eprintln!("[chronikl:debug] telemetry POST {TELEMETRY_URL}");
    match serde_json::to_string_pretty(payload) {
        Ok(json) => {
            for line in json.lines() {
                eprintln!("[chronikl:debug]   {line}");
            }
        }
        Err(e) => eprintln!("[chronikl:debug] failed to serialise payload: {e}"),
    }
    let client = match crate::http::build_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[chronikl:debug] failed to build HTTP client: {e}");
            return;
        }
    };
    match client.post(TELEMETRY_URL).json(payload).send().await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            eprintln!("[chronikl:debug] response: {status}");
            if !body.is_empty() {
                eprintln!("[chronikl:debug] body: {body}");
            }
        }
        Err(e) => {
            eprintln!("[chronikl:debug] request failed: {e}");
            let mut source = std::error::Error::source(&e);
            while let Some(cause) = source {
                eprintln!("[chronikl:debug]   caused by: {cause}");
                source = std::error::Error::source(cause);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> HeartbeatPayload {
        HeartbeatPayload::from_release(
            10,
            TierCounts {
                tier0: 10,
                tier1: 4,
                tier2: 1,
                tier3: 0,
                cache_hits: 5,
            },
            Some("anthropic".into()),
            Some("claude-sonnet-4-6".into()),
            "rebase",
            false,                 // prerelease
            "minor",               // version_bump
            Some("semver".into()), // version_scheme
            false,                 // custom_voice
            false,                 // agent_used
            true,                  // pr_enrichment
            false,                 // audit_log
            false,                 // licensed
            true,                  // success
        )
    }

    #[test]
    fn payload_round_trips_through_json() {
        let p = sample();
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["commit_count"], 10);
        assert_eq!(json["tier_counts"]["tier1"], 4);
        assert_eq!(json["tier_counts"]["cache_hits"], 5);
        assert_eq!(json["provider"], "anthropic");
        assert_eq!(json["model"], "claude-sonnet-4-6");
        assert_eq!(json["merge_style"], "rebase");
        assert_eq!(json["custom_voice"], false);
        assert_eq!(json["pr_enrichment"], true);
        assert_eq!(json["success"], true);
        // run_id is a valid UUID
        uuid::Uuid::parse_str(json["run_id"].as_str().unwrap())
            .expect("run_id should be a valid UUID");
    }

    #[test]
    fn payload_omits_provider_and_model_when_none() {
        let p = HeartbeatPayload::from_release(
            3,
            TierCounts::default(),
            None,
            None,
            "rebase",
            false,
            "unknown",
            None,
            false,
            false,
            false,
            false,
            false,
            true,
        );
        let json = serde_json::to_value(&p).unwrap();
        // skip_serializing_if drops the keys entirely.
        assert!(
            !json.as_object().unwrap().contains_key("provider"),
            "provider should be omitted when None"
        );
        assert!(
            !json.as_object().unwrap().contains_key("model"),
            "model should be omitted when None"
        );
    }

    #[test]
    fn payload_excludes_user_text() {
        // Pin the schema: any future field that smells like user data
        // should fail this test until added to the allowlist below.
        let p = sample();
        let allowed: std::collections::HashSet<&str> = [
            "run_id",
            "commit_count",
            "tier_counts",
            "provider",
            "model",
            "merge_style",
            "prerelease",
            "version_bump",
            "version_scheme",
            "custom_voice",
            "agent_used",
            "pr_enrichment",
            "audit_log",
            "licensed",
            "is_ci",
            "success",
            "version",
        ]
        .into_iter()
        .collect();
        let json = serde_json::to_value(&p).unwrap();
        for key in json.as_object().unwrap().keys() {
            assert!(
                allowed.contains(key.as_str()),
                "unexpected payload field `{key}` — if you're adding a new field, \
                 confirm it carries no user text and add it to the test allowlist"
            );
        }
    }

    #[test]
    fn is_debug_returns_a_bool() {
        let _ = is_debug();
    }

    #[tokio::test]
    async fn send_heartbeat_does_not_panic_on_unreachable_url() {
        let p = sample();
        std::mem::drop(send_heartbeat(p));
        // Give the spawned task a moment to attempt the POST and silently
        // fail. The point is the runtime + spawn machinery is wired
        // correctly; we don't await — drops are fine for fire-and-forget.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
