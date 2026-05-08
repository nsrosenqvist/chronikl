//! End-to-end with a mock provider: real git repo → Tier 0 → Tier 1
//! against a canned LLM response → audit log.
//!
//! Validates the full LLM-aware pipeline without making any network
//! calls. Always runs in CI.

use std::path::Path;
use std::process::Command;

use chronikl::audit::{AuditSink, CallStatus};
use chronikl::git::{self, RangeSpec};
use chronikl::ladder::{tier0, tier1};
use chronikl::models::{ClassificationSource, Section};
use chronikl::providers::NotesProvider;
use chronikl::providers::mock::MockProvider;
use tempfile::TempDir;

struct Repo {
    _dir: TempDir,
    path: std::path::PathBuf,
}

impl Repo {
    fn init() -> Self {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        run(&path, &["init", "--initial-branch=main"]);
        run(&path, &["config", "user.name", "Ada Lovelace"]);
        run(&path, &["config", "user.email", "ada@example.com"]);
        run(&path, &["config", "commit.gpgsign", "false"]);
        Self { _dir: dir, path }
    }
    fn write(&self, rel: &str, contents: &str) {
        let full = self.path.join(rel);
        if let Some(p) = full.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(full, contents).unwrap();
    }
    fn commit(&self, subject: &str) {
        run(&self.path, &["add", "-A"]);
        run(&self.path, &["commit", "-m", subject]);
    }
    fn tag(&self, t: &str) {
        run(&self.path, &["tag", t]);
    }
}

fn run(repo: &Path, args: &[&str]) {
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

#[tokio::test]
async fn full_pipeline_with_mock_provider_records_audit() {
    let repo = Repo::init();
    repo.write("README.md", "v0\n");
    repo.commit("chore: seed");
    repo.tag("v0.1.0");

    // One Tier-0-classifiable commit, two that should fall through to Tier 1.
    repo.write("src/login.rs", "fn login() {}\n");
    repo.commit("feat(auth): add login endpoint");

    repo.write("src/cache.rs", "fn cache() {}\n");
    repo.commit("wip"); // No conventional prefix → Tier 0 leaves it in Other.

    repo.write("src/utils.rs", "fn util() {}\n");
    repo.commit("yet another raw commit"); // Same.

    let resolved = git::resolve_range(
        &repo.path,
        &RangeSpec::Explicit {
            from: Some("v0.1.0".into()),
            to: "HEAD".into(),
        },
    )
    .await
    .unwrap();
    let commits = git::log(&repo.path, &resolved).await.unwrap();
    let enriched = chronikl::models::EnrichedCommit::from_commits(commits.clone());
    let mut classified = tier0::classify(&enriched);

    // The mock returns a single batch verdict that places both
    // unclassified commits into BugFixes.
    let verdicts = serde_json::json!({
        "verdicts": [
            {"index": 0, "section": "bug-fixes", "summary": "Recover from cache misses", "confidence": 0.85},
            {"index": 1, "section": "refactor", "summary": "Tidy utility helpers", "confidence": 0.7}
        ]
    });
    let audit = AuditSink::new();
    audit.enable();
    let provider = MockProvider::returning(verdicts.to_string()).with_audit_sink(audit.clone());

    let updated = tier1::classify(
        &mut classified,
        &provider as &dyn NotesProvider,
        &audit,
        50,
        1.0,
        tier1::default_system_prompt(),
    )
    .await
    .unwrap();
    assert_eq!(updated, 2);

    // The Tier-0-classified `feat` commit was untouched.
    let auth_commit = classified
        .iter()
        .find(|c| c.commit.subject.contains("login endpoint"))
        .unwrap();
    assert_eq!(auth_commit.classification.section, Section::Features);
    assert_eq!(auth_commit.classification.confidence, 1.0);

    // git log is reverse-chronological: "yet another" was committed
    // last, so it lands at batch index 0 and gets the BugFixes verdict.
    // "wip" is older, batch index 1, gets Refactor.
    let raw = classified
        .iter()
        .find(|c| c.commit.subject.starts_with("yet another"))
        .unwrap();
    assert_eq!(raw.classification.section, Section::BugFixes);
    assert!(matches!(
        raw.classification.source,
        ClassificationSource::BatchedLlm
    ));
    assert!((raw.classification.confidence - 0.85).abs() < 1e-5);

    let wip = classified
        .iter()
        .find(|c| c.commit.subject == "wip")
        .unwrap();
    assert_eq!(wip.classification.section, Section::Refactor);

    // Audit captured the call with both SHAs.
    let document = audit.finalize(classified, "rendered placeholder".into());
    assert_eq!(document.llm_calls.len(), 1);
    let call = &document.llm_calls[0];
    assert!(matches!(call.status, CallStatus::Success));
    assert_eq!(call.commit_shas.len(), 2);
    assert!(!call.prompt_hash.is_empty());
    assert!(call.response_hash.is_some());
}

#[tokio::test]
async fn provider_failure_propagates_and_audit_records_failure() {
    let repo = Repo::init();
    repo.write("a", "1\n");
    repo.commit("seed");
    repo.tag("v0.1.0");
    repo.write("b", "1\n");
    repo.commit("wip"); // raw commit → Tier 0 → Other → Tier 1 will retry.

    let resolved = git::resolve_range(
        &repo.path,
        &RangeSpec::Explicit {
            from: Some("v0.1.0".into()),
            to: "HEAD".into(),
        },
    )
    .await
    .unwrap();
    let commits = git::log(&repo.path, &resolved).await.unwrap();
    let enriched = chronikl::models::EnrichedCommit::from_commits(commits.clone());
    let mut classified = tier0::classify(&enriched);

    let audit = AuditSink::new();
    audit.enable();
    let provider = MockProvider::failing("simulated 500").with_audit_sink(audit.clone());

    let err = tier1::classify(
        &mut classified,
        &provider as &dyn NotesProvider,
        &audit,
        50,
        1.0,
        tier1::default_system_prompt(),
    )
    .await
    .unwrap_err();
    assert!(format!("{err:#}").contains("simulated 500"));

    let document = audit.finalize(classified, String::new());
    assert_eq!(document.llm_calls.len(), 1);
    assert!(matches!(
        document.llm_calls[0].status,
        CallStatus::Failed { .. }
    ));
}

#[tokio::test]
async fn no_pending_commits_skips_provider_entirely() {
    let repo = Repo::init();
    repo.write("a", "1\n");
    repo.commit("seed");
    repo.tag("v0.1.0");

    // All conventional → all Tier-0-classified at confidence 1.0.
    repo.write("b", "1\n");
    repo.commit("feat: add b");
    repo.write("c", "1\n");
    repo.commit("fix: c");

    let resolved = git::resolve_range(
        &repo.path,
        &RangeSpec::Explicit {
            from: Some("v0.1.0".into()),
            to: "HEAD".into(),
        },
    )
    .await
    .unwrap();
    let commits = git::log(&repo.path, &resolved).await.unwrap();
    let enriched = chronikl::models::EnrichedCommit::from_commits(commits.clone());
    let mut classified = tier0::classify(&enriched);

    // If the provider is touched at all, the test fails (the canned
    // response is not a valid verdict shape).
    let audit = AuditSink::new();
    audit.enable();
    let provider =
        MockProvider::returning("THIS WOULD CAUSE A PARSE ERROR").with_audit_sink(audit.clone());

    let updated = tier1::classify(
        &mut classified,
        &provider as &dyn NotesProvider,
        &audit,
        50,
        1.0,
        tier1::default_system_prompt(),
    )
    .await
    .unwrap();
    assert_eq!(updated, 0);
    assert_eq!(provider.call_count(), 0);
}
