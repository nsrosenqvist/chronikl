//! Tier 2 integration: real git repo → real `git show` for diffs →
//! mock provider that returns canned per-commit verdicts → assert
//! `Classified` was updated and audit captured per-commit calls.

use std::path::Path;
use std::process::Command;

use chronikl::audit::AuditSink;
use chronikl::git::{self, RangeSpec};
use chronikl::ladder::{tier0, tier2};
use chronikl::models::{ClassificationSource, Section};
use chronikl::providers::NotesProvider;
use chronikl::providers::mock::MockProvider;
use tempfile::TempDir;

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
    run_git(&path, &["config", "user.name", "Ada"]);
    run_git(&path, &["config", "user.email", "ada@x"]);
    run_git(&path, &["config", "commit.gpgsign", "false"]);

    std::fs::write(path.join("README.md"), "v0\n").unwrap();
    run_git(&path, &["add", "."]);
    run_git(&path, &["commit", "-m", "chore: seed"]);
    run_git(&path, &["tag", "v0.1.0"]);

    // Two ambiguous commits that Tier 0 leaves in `Other` with low
    // confidence — both are candidates for Tier 2.
    std::fs::write(path.join("src.rs"), "fn auth() {}\n").unwrap();
    run_git(&path, &["add", "."]);
    run_git(&path, &["commit", "-m", "wip auth thing"]);

    std::fs::write(path.join("api.rs"), "// removed: pub fn old_api()\n").unwrap();
    run_git(&path, &["add", "."]);
    run_git(&path, &["commit", "-m", "cleanup"]);

    (dir, path)
}

#[tokio::test]
async fn tier2_runs_per_commit_and_updates_classification() {
    let (_dir, repo) = make_repo();

    let resolved = git::resolve_range(
        &repo,
        &RangeSpec::Explicit {
            from: Some("v0.1.0".into()),
            to: "HEAD".into(),
        },
    )
    .await
    .unwrap();
    let commits = git::log(&repo, &resolved).await.unwrap();
    let enriched = chronikl::models::EnrichedCommit::from_commits(commits.clone());
    let mut classified = tier0::classify(&enriched);

    // Two responses — one per commit. Tier 2 walks pending commits in
    // input order (which is reverse-chronological from `git log`).
    // Call 0: latest commit ("cleanup") → classify as breaking via the
    //         model's `breaking: true` signal.
    // Call 1: earlier commit ("wip auth thing") → BugFixes, low conf.
    let r0 = serde_json::json!({
        "section": "refactor",
        "summary": "Remove deprecated old_api",
        "confidence": 0.9,
        "breaking": true,
    });
    let r1 = serde_json::json!({
        "section": "bug-fixes",
        "summary": "Fix auth handler",
        "confidence": 0.7,
        "breaking": false,
    });
    let provider = MockProvider::returning_sequence(vec![r0.to_string(), r1.to_string()]);

    let audit = AuditSink::new();
    audit.enable();

    let updated = tier2::classify(
        &mut classified,
        &repo,
        &provider as &dyn NotesProvider,
        &audit,
        4000,
        0.6,
        tier2::default_system_prompt(),
    )
    .await
    .unwrap();
    assert_eq!(updated, 2);
    assert_eq!(provider.call_count(), 2);

    // The `breaking: true` flag forced section=Breaking even though the
    // model said "refactor".
    let cleanup = classified
        .iter()
        .find(|c| c.commit.subject == "cleanup")
        .unwrap();
    assert_eq!(cleanup.classification.section, Section::Breaking);
    assert_eq!(cleanup.classification.summary, "Remove deprecated old_api");
    assert!(matches!(
        cleanup.classification.source,
        ClassificationSource::PerCommitLlm
    ));

    let wip = classified
        .iter()
        .find(|c| c.commit.subject == "wip auth thing")
        .unwrap();
    assert_eq!(wip.classification.section, Section::BugFixes);
    assert_eq!(wip.classification.summary, "Fix auth handler");

    // Each Tier 2 call should have included the diff in the user prompt.
    let calls = provider.captured_calls();
    for call in &calls {
        assert!(
            call.user_prompt.contains("<diff>"),
            "user prompt should include the diff section: {}",
            call.user_prompt
        );
    }
    // The most recent call's prompt should reference the api.rs diff.
    assert!(calls[0].user_prompt.contains("api.rs"));
}

#[tokio::test]
async fn tier2_skips_high_confidence_commits() {
    let (_dir, repo) = make_repo();

    let resolved = git::resolve_range(
        &repo,
        &RangeSpec::Explicit {
            from: Some("v0.1.0".into()),
            to: "HEAD".into(),
        },
    )
    .await
    .unwrap();
    let commits = git::log(&repo, &resolved).await.unwrap();
    let enriched = chronikl::models::EnrichedCommit::from_commits(commits.clone());
    let mut classified = tier0::classify(&enriched);

    // Boost both commits' confidence above the threshold so Tier 2
    // shouldn't touch them.
    for entry in classified.0.iter_mut() {
        entry.classification.confidence = 0.95;
    }

    let provider = MockProvider::returning("THIS WOULD BREAK PARSING");
    let audit = AuditSink::new();

    let updated = tier2::classify(
        &mut classified,
        &repo,
        &provider as &dyn NotesProvider,
        &audit,
        4000,
        0.6,
        tier2::default_system_prompt(),
    )
    .await
    .unwrap();
    assert_eq!(updated, 0);
    assert_eq!(provider.call_count(), 0);
}

#[tokio::test]
async fn tier2_truncates_oversized_diff() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_path_buf();
    run_git(&path, &["init", "--initial-branch=main"]);
    run_git(&path, &["config", "user.name", "Ada"]);
    run_git(&path, &["config", "user.email", "ada@x"]);
    run_git(&path, &["config", "commit.gpgsign", "false"]);

    std::fs::write(path.join("README.md"), "v0\n").unwrap();
    run_git(&path, &["add", "."]);
    run_git(&path, &["commit", "-m", "chore: seed"]);
    run_git(&path, &["tag", "v0.1.0"]);

    // Generate a large diff so the truncation marker fires.
    let big = "x".repeat(80_000);
    std::fs::write(path.join("big.txt"), &big).unwrap();
    run_git(&path, &["add", "."]);
    run_git(&path, &["commit", "-m", "wip"]);

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

    let r = serde_json::json!({
        "section": "chore",
        "summary": "Add a large fixture file",
        "confidence": 0.5,
        "breaking": false,
    });
    let provider = MockProvider::returning(r.to_string());
    let audit = AuditSink::new();

    tier2::classify(
        &mut classified,
        &path,
        &provider as &dyn NotesProvider,
        &audit,
        100, // small budget — diff must get truncated
        0.6,
        tier2::default_system_prompt(),
    )
    .await
    .unwrap();

    let calls = provider.captured_calls();
    assert!(
        calls[0].user_prompt.contains("diff truncated"),
        "expected truncation marker in prompt: {}",
        calls[0].user_prompt
    );
}

#[tokio::test]
async fn breaking_flag_overrides_section() {
    let (_dir, repo) = make_repo();

    let resolved = git::resolve_range(
        &repo,
        &RangeSpec::Explicit {
            from: Some("v0.1.0".into()),
            to: "HEAD".into(),
        },
    )
    .await
    .unwrap();
    let commits = git::log(&repo, &resolved).await.unwrap();
    let enriched = chronikl::models::EnrichedCommit::from_commits(commits.clone());
    let mut classified = tier0::classify(&enriched);

    // Both commits return breaking=true with a non-Breaking section
    // string. Post-processing should force them into Breaking.
    let r = serde_json::json!({
        "section": "features",
        "summary": "Drop support for v1",
        "confidence": 0.9,
        "breaking": true,
    });
    let provider = MockProvider::new(
        chronikl::providers::mock::MockProviderConfig {
            provider_name: "mock".into(),
            model: "mock".into(),
            audit_sink: None,
        },
        chronikl::providers::mock::MockResponse::Static(r.to_string()),
    );
    let audit = AuditSink::new();

    tier2::classify(
        &mut classified,
        &repo,
        &provider as &dyn NotesProvider,
        &audit,
        4000,
        0.6,
        tier2::default_system_prompt(),
    )
    .await
    .unwrap();

    for entry in classified.iter() {
        assert_eq!(entry.classification.section, Section::Breaking);
    }
}
