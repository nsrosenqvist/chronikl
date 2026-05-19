//! End-to-end tests against a real LLM provider.
//!
//! `#[ignore]`d by default — run with one of the supported keys set:
//!
//! ```bash
//! ANTHROPIC_API_KEY=sk-ant-...  cargo test --test e2e_profiles -- --ignored --nocapture
//! OPENAI_API_KEY=sk-...         cargo test --test e2e_profiles -- --ignored --nocapture
//! GEMINI_API_KEY=AIza...        cargo test --test e2e_profiles -- --ignored --nocapture
//!
//! # Or override explicitly via the chronikl env-var family:
//! CHRONIKL_PROVIDER=gemini \
//! CHRONIKL_API_KEY=AIza... \
//!   cargo test --test e2e_profiles -- --ignored --nocapture
//! ```
//!
//! Skips at runtime (rather than panicking) if no provider is
//! configured, so `cargo test -- --include-ignored` on a machine with
//! no keys still passes. Mirrors nitpik's `tests/e2e_profiles.rs`
//! pattern.

use std::path::Path;
use std::process::Command;

use chronikl::audit::{AuditSink, CallStatus};
use chronikl::config::ProviderConfig;
use chronikl::git::{self, RangeSpec};
use chronikl::ladder::{tier0, tier1, tier2, tier3};
use chronikl::models::{MergeStyle, ReleaseKind, Section, VersionBump};
use chronikl::project::ProjectContext;
use chronikl::prose::{self, ProseRequest};
use chronikl::providers::NotesProvider;
use chronikl::providers::rig::RigProvider;
use chronikl::voice;
use tempfile::TempDir;

/// Resolve the provider config the E2E tests should run against.
///
/// Priority:
///   1. Explicit `CHRONIKL_PROVIDER` + `CHRONIKL_API_KEY` (allows
///      arbitrary providers, including `openai-compatible`).
///   2. Provider-specific keys in order: Anthropic → OpenAI → Gemini.
///
/// Returns `None` when nothing is configured — tests skip gracefully.
fn e2e_provider() -> Option<ProviderConfig> {
    if let (Ok(name), Ok(key)) = (
        std::env::var("CHRONIKL_PROVIDER"),
        std::env::var("CHRONIKL_API_KEY"),
    ) {
        let model = std::env::var("CHRONIKL_E2E_MODEL")
            .ok()
            .or_else(|| std::env::var("CHRONIKL_MODEL").ok())
            .or_else(|| default_model(&name));
        return Some(ProviderConfig {
            name: Some(name),
            model,
            api_key: Some(key),
            base_url: std::env::var("CHRONIKL_BASE_URL").ok(),
        });
    }
    for (env_var, provider) in [
        ("ANTHROPIC_API_KEY", "anthropic"),
        ("OPENAI_API_KEY", "openai"),
        ("GEMINI_API_KEY", "gemini"),
    ] {
        if let Ok(key) = std::env::var(env_var) {
            return Some(ProviderConfig {
                name: Some(provider.into()),
                model: Some(default_model(provider).expect("known provider has default model")),
                api_key: Some(key),
                base_url: None,
            });
        }
    }
    None
}

/// Cheap + fast default model per provider — quality bar for E2E is
/// "did structured output round-trip", not "is the prose great".
/// Override with `CHRONIKL_E2E_MODEL` when you need a specific one.
fn default_model(provider: &str) -> Option<String> {
    Some(
        match provider {
            "anthropic" => "claude-haiku-4-5-20251001",
            "openai" => "gpt-4.1-mini",
            "gemini" => "gemini-2.5-flash",
            _ => return None,
        }
        .to_string(),
    )
}

/// Resolve the provider and emit a banner so `--nocapture` runs make
/// it obvious which provider answered. Returns `None` when nothing is
/// configured — callers should skip in that case.
fn provider_or_skip() -> Option<ProviderConfig> {
    let cfg = e2e_provider()?;
    eprintln!(
        "[e2e] using provider={} model={}",
        cfg.name.as_deref().unwrap_or("?"),
        cfg.model.as_deref().unwrap_or("?"),
    );
    Some(cfg)
}

fn run_git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    if !out.status.success() {
        panic!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn make_repo() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_path_buf();
    run_git(&path, &["init", "--initial-branch=main"]);
    run_git(&path, &["config", "user.name", "Ada Lovelace"]);
    run_git(&path, &["config", "user.email", "ada@example.com"]);
    run_git(&path, &["config", "commit.gpgsign", "false"]);

    std::fs::write(path.join("README.md"), "v0\n").unwrap();
    run_git(&path, &["add", "."]);
    run_git(&path, &["commit", "-m", "chore: initial"]);
    run_git(&path, &["tag", "v0.1.0"]);

    // Three intentionally raw commits that Tier 0 leaves in `Other`.
    std::fs::write(path.join("src.rs"), "fn auth() {}\n").unwrap();
    run_git(&path, &["add", "."]);
    run_git(&path, &["commit", "-m", "Wire up the auth handler at last"]);

    std::fs::write(path.join("util.rs"), "fn util() {}\n").unwrap();
    run_git(&path, &["add", "."]);
    run_git(&path, &["commit", "-m", "Stop crashing on empty cache"]);

    std::fs::write(path.join("docs.txt"), "...\n").unwrap();
    run_git(&path, &["add", "."]);
    run_git(&path, &["commit", "-m", "wip"]);

    (dir, path)
}

#[tokio::test]
#[ignore = "needs ANTHROPIC_API_KEY / OPENAI_API_KEY / GEMINI_API_KEY"]
async fn classifies_a_real_batch() {
    let Some(provider_cfg) = provider_or_skip() else {
        eprintln!("no E2E provider configured; skipping");
        return;
    };

    let (_dir, repo_path) = make_repo();
    let resolved = git::resolve_range(
        &repo_path,
        &RangeSpec::Explicit {
            from: Some("v0.1.0".into()),
            to: "HEAD".into(),
        },
    )
    .await
    .unwrap();
    let commits = git::log(&repo_path, &resolved).await.unwrap();
    assert_eq!(commits.len(), 3);
    let enriched = chronikl::models::EnrichedCommit::from_commits(commits.clone());
    let mut classified = tier0::classify(&enriched);
    // Pre-condition: Tier 0 leaves all three in Other.
    for c in classified.iter() {
        assert_eq!(
            c.classification.section,
            Section::Other,
            "Tier 0 unexpectedly classified `{}`",
            c.commit.subject
        );
    }

    let audit = AuditSink::new();
    audit.enable();

    let provider = RigProvider::new(provider_cfg)
        .expect("RigProvider for Anthropic")
        .with_audit_sink(audit.clone());

    let updated = tier1::classify(
        &mut classified,
        &provider as &dyn NotesProvider,
        &audit,
        50,
        1.0,
        tier1::default_system_prompt(),
    )
    .await
    .expect("tier 1 against real Anthropic");
    assert_eq!(updated, 3);

    // Spot-check: the three commits should now have non-Other sections.
    let auth = classified
        .iter()
        .find(|c| c.commit.subject.contains("auth handler"))
        .unwrap();
    let cache = classified
        .iter()
        .find(|c| c.commit.subject.contains("empty cache"))
        .unwrap();
    let wip = classified
        .iter()
        .find(|c| c.commit.subject == "wip")
        .unwrap();

    eprintln!("auth → {:?}", auth.classification);
    eprintln!("cache → {:?}", cache.classification);
    eprintln!("wip → {:?}", wip.classification);

    // We don't pin to specific sections — different model versions
    // pick differently. We do require the placements to no longer be
    // the default `Other` (i.e. the model engaged) for the two
    // substantive commits.
    assert_ne!(
        auth.classification.section,
        Section::Other,
        "Anthropic should classify the auth commit"
    );
    assert_ne!(
        cache.classification.section,
        Section::Other,
        "Anthropic should classify the cache commit"
    );
    // The `wip` commit may legitimately stay in Other with low
    // confidence — that's correct behaviour.
    assert!(wip.classification.confidence <= 1.0);

    // Audit log captured a successful call with non-zero token usage.
    let document = audit.finalize(classified, String::new());
    assert!(!document.llm_calls.is_empty());
    let call = &document.llm_calls[0];
    assert!(matches!(call.status, CallStatus::Success));
    assert!(
        call.tokens.input_tokens > 0,
        "expected real input tokens to be reported, got {:?}",
        call.tokens
    );
    assert!(call.tokens.output_tokens > 0);
    assert_eq!(call.commit_shas.len(), 3);
}

#[tokio::test]
#[ignore = "needs ANTHROPIC_API_KEY / OPENAI_API_KEY / GEMINI_API_KEY"]
async fn tier2_per_commit_with_diff() {
    let Some(provider_cfg) = provider_or_skip() else {
        eprintln!("no E2E provider configured; skipping");
        return;
    };

    let (_dir, repo_path) = make_repo();
    let resolved = git::resolve_range(
        &repo_path,
        &RangeSpec::Explicit {
            from: Some("v0.1.0".into()),
            to: "HEAD".into(),
        },
    )
    .await
    .unwrap();
    let commits = git::log(&repo_path, &resolved).await.unwrap();
    let enriched = chronikl::models::EnrichedCommit::from_commits(commits.clone());
    let mut classified = tier0::classify(&enriched);

    let audit = AuditSink::new();
    audit.enable();

    let provider = RigProvider::new(provider_cfg)
        .expect("RigProvider for Anthropic")
        .with_audit_sink(audit.clone());

    // Run Tier 1 first.
    tier1::classify(
        &mut classified,
        &provider as &dyn NotesProvider,
        &audit,
        50,
        1.0,
        tier1::default_system_prompt(),
    )
    .await
    .expect("tier 1");

    // Force Tier 2 to escalate everything by setting a near-1.0
    // threshold. Tier 1 results are usually 0.6–0.9, so all of them
    // get re-classified with the diff.
    let updated_t2 = tier2::classify(
        &mut classified,
        &repo_path,
        &provider as &dyn NotesProvider,
        &audit,
        4000,
        0.99,
        tier2::default_system_prompt(),
    )
    .await
    .expect("tier 2 against real Anthropic");
    assert!(
        updated_t2 >= 1,
        "Tier 2 should reclassify at least one commit"
    );

    // Audit captured both Tier 1 and Tier 2 calls.
    let document = audit.finalize(classified, String::new());
    let labels: Vec<&str> = document
        .llm_calls
        .iter()
        .map(|c| c.label.as_str())
        .collect();
    assert!(
        labels.iter().any(|l| l.starts_with("tier-1")),
        "expected at least one tier-1 call, got {labels:?}"
    );
    assert!(
        labels.iter().any(|l| l == &"tier-2-per-commit"),
        "expected at least one tier-2-per-commit call, got {labels:?}"
    );
    // Each Tier 2 call should be tagged with exactly one commit SHA.
    for call in &document.llm_calls {
        if call.label == "tier-2-per-commit" {
            assert_eq!(
                call.commit_shas.len(),
                1,
                "Tier 2 audit record should carry exactly one SHA"
            );
        }
    }
}

#[tokio::test]
#[ignore = "needs ANTHROPIC_API_KEY / OPENAI_API_KEY / GEMINI_API_KEY"]
async fn prose_pass_writes_grouped_markdown() {
    let Some(provider_cfg) = provider_or_skip() else {
        eprintln!("no E2E provider configured; skipping");
        return;
    };

    let (_dir, repo_path) = make_repo();
    let resolved = git::resolve_range(
        &repo_path,
        &RangeSpec::Explicit {
            from: Some("v0.1.0".into()),
            to: "HEAD".into(),
        },
    )
    .await
    .unwrap();
    let commits = git::log(&repo_path, &resolved).await.unwrap();
    let enriched = chronikl::models::EnrichedCommit::from_commits(commits.clone());
    let mut classified = tier0::classify(&enriched);

    let audit = AuditSink::new();
    audit.enable();

    let provider = RigProvider::new(provider_cfg)
        .expect("RigProvider for Anthropic")
        .with_audit_sink(audit.clone());

    // Tier 1 first to get the commits out of `Other`.
    tier1::classify(
        &mut classified,
        &provider as &dyn NotesProvider,
        &audit,
        50,
        1.0,
        tier1::default_system_prompt(),
    )
    .await
    .expect("tier 1");

    let voice_obj = voice::default();
    let md = prose::run(
        ProseRequest {
            voice: &voice_obj,
            extra_instructions: None,
            inline_prompt: Some("Be terse. Write in past tense."),
            classified: &classified,
            merge_style: MergeStyle::Rebase,
            release_kind: &ReleaseKind::Stable,
            version_bump: VersionBump::Unknown,
            version_scheme: None,
            from_ref: Some("v0.1.0"),
            to_ref: "HEAD",
            rich_context: false,
            project_context: &ProjectContext::default(),
        },
        &provider as &dyn NotesProvider,
        &audit,
    )
    .await
    .expect("prose pass against real Anthropic");

    eprintln!("--- prose pass output ---\n{md}\n--- end ---");

    // The model should produce non-empty Markdown with at least one
    // section header. We don't pin the exact wording — voice + sampling
    // makes that flaky.
    assert!(!md.trim().is_empty(), "prose output should not be empty");
    assert!(
        md.contains("###") || md.contains("##"),
        "expected at least one section header in output: {md}"
    );

    // Audit captured the prose call tagged with all commit SHAs.
    let document = audit.finalize(classified, md);
    let prose_call = document
        .llm_calls
        .iter()
        .find(|c| c.label == "prose")
        .expect("prose audit record");
    assert!(matches!(prose_call.status, CallStatus::Success));
    assert_eq!(prose_call.commit_shas.len(), 3);
    assert!(prose_call.tokens.input_tokens > 0);
    assert!(prose_call.tokens.output_tokens > 0);
}

#[tokio::test]
#[ignore = "needs ANTHROPIC_API_KEY / OPENAI_API_KEY / GEMINI_API_KEY"]
async fn tier3_agent_loop_with_tools() {
    let Some(provider_cfg) = provider_or_skip() else {
        eprintln!("no E2E provider configured; skipping");
        return;
    };

    // Build a fixture repo where the model needs to *look at the repo*
    // to figure out what a commit really did. The commit subject is
    // intentionally cryptic; the diff just removes a function. The
    // model should call list_directory or read_file to confirm.
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_path_buf();
    run_git(&path, &["init", "--initial-branch=main"]);
    run_git(&path, &["config", "user.name", "Ada"]);
    run_git(&path, &["config", "user.email", "ada@x"]);
    run_git(&path, &["config", "commit.gpgsign", "false"]);

    std::fs::write(
        path.join("legacy.rs"),
        "pub fn legacy_login() {}\npub fn legacy_logout() {}\n",
    )
    .unwrap();
    std::fs::write(path.join("README.md"), "see docs/legacy.md\n").unwrap();
    run_git(&path, &["add", "."]);
    run_git(&path, &["commit", "-m", "chore: seed"]);
    run_git(&path, &["tag", "v0.1.0"]);

    // Cryptic commit removing public API.
    std::fs::write(path.join("legacy.rs"), "// removed legacy interfaces\n").unwrap();
    run_git(&path, &["add", "."]);
    run_git(&path, &["commit", "-m", "cleanup"]);

    let resolved = git::resolve_range(
        &path,
        &RangeSpec::Explicit {
            from: Some("v0.1.0".into()),
            to: "HEAD".into(),
        },
    )
    .await
    .unwrap();
    let commits = git::log(&path, &resolved).await.unwrap();
    let enriched = chronikl::models::EnrichedCommit::from_commits(commits.clone());
    let mut classified = tier0::classify(&enriched);

    let audit = AuditSink::new();
    audit.enable();

    let provider = RigProvider::new(provider_cfg)
        .expect("RigProvider for Anthropic")
        .with_audit_sink(audit.clone());

    let updated = tier3::classify(
        &mut classified,
        &path,
        &provider,
        &audit,
        4000,
        0.99, // force escalation regardless of Tier 0/1/2 confidence
        6,    // turn cap for the test
        tier3::default_system_prompt(),
    )
    .await
    .expect("tier 3 against real Anthropic");
    assert!(
        updated >= 1,
        "Tier 3 should classify at least one commit, got {updated}"
    );

    // Audit must have a tier-3-agent record with full diagnostics.
    let document = audit.finalize(classified, String::new());
    let agent_call = document
        .llm_calls
        .iter()
        .find(|c| c.label == "tier-3-agent")
        .expect("tier-3-agent audit record");
    assert!(matches!(agent_call.status, CallStatus::Success));
    let diag = agent_call
        .agent
        .as_ref()
        .expect("agent diagnostics on Tier 3 record");
    assert!(diag.turns >= 1, "agent loop ran at least one turn");
    eprintln!(
        "Tier 3: turns={} terminated_via={:?} self_repair={} tool_calls={}",
        diag.turns,
        diag.terminated_via_tool,
        diag.self_repair_attempted,
        diag.tool_calls.len()
    );
    for tc in &diag.tool_calls {
        eprintln!(
            "  tool: {} args={} result={} ({}ms{})",
            tc.name,
            tc.args_summary,
            tc.result_summary,
            tc.duration_ms,
            if tc.failed { " FAILED" } else { "" }
        );
    }
}
