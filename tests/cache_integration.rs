//! Cache integration: real disk roundtrip + ladder interaction.
//!
//! Verifies the disk cache survives across "runs" and that a populated
//! cache lets the ladder skip the LLM entirely.

use chronikl::audit::AuditSink;
use chronikl::cache::{ClassificationCache, MemoryCache, disk::DiskCache};
use chronikl::ladder::tier1;
use chronikl::models::{
    Classification, ClassificationSource, Classified, ClassifiedCommit, Commit, Section,
};
use chronikl::providers::NotesProvider;
use chronikl::providers::mock::MockProvider;
use std::path::Path;
use std::process::Command;
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

fn make_commit(sha: &str, subject: &str) -> ClassifiedCommit {
    ClassifiedCommit {
        commit: Commit {
            sha: sha.into(),
            short_sha: sha[..7.min(sha.len())].into(),
            author_name: "ada".into(),
            author_email: "ada@x".into(),
            author_date: "2026-01-01T00:00:00+00:00".into(),
            parents: vec!["p".into()],
            subject: subject.into(),
            body: String::new(),
            files: vec!["src.rs".into()],
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

#[test]
fn disk_cache_persists_across_handles() {
    let dir = TempDir::new().unwrap();
    let key_sha = "abcd1234567890abcdef1234567890abcdef0000";
    let model = "claude-sonnet-4-6";

    // Handle 1: write.
    let cache_a = DiskCache::new(dir.path().to_path_buf());
    cache_a.put(
        key_sha,
        model,
        &Classification {
            section: Section::Features,
            summary: "Add login".into(),
            source: ClassificationSource::BatchedLlm,
            confidence: 0.85,
        },
    );

    // Handle 2: read.
    let cache_b = DiskCache::new(dir.path().to_path_buf());
    let got = cache_b.get(key_sha, model).unwrap();
    assert_eq!(got.summary, "Add login");
    assert_eq!(got.confidence, 0.85);
}

#[tokio::test]
async fn populated_cache_skips_llm() {
    // Set up: two commits, one cached, one not.
    let mut classified = Classified(vec![
        make_commit("aaa", "wip"),
        make_commit("bbb", "another raw commit"),
    ]);

    let cache = MemoryCache::new();
    let model = "test-model";
    // Cache at confidence 1.0 — the previous run reached high-confidence,
    // so this run shouldn't re-classify it. (Lower-confidence cached
    // results legitimately *can* be re-escalated by later tiers; that's
    // the design, but it's a different test.)
    cache.put(
        "aaa",
        model,
        &Classification {
            section: Section::Features,
            summary: "Cached: add login".into(),
            source: ClassificationSource::BatchedLlm,
            confidence: 1.0,
        },
    );

    // Simulate the main flow: populate before Tier 1, then run Tier 1.
    // Only one commit (`bbb`) should hit the provider.
    cache.populate(&mut classified, model);

    // After populate: aaa now has confidence=1.0 (cached); bbb still 0.5.
    assert_eq!(classified.0[0].classification.confidence, 1.0);
    assert!(matches!(
        classified.0[0].classification.source,
        ClassificationSource::BatchedLlm
    ));
    assert_eq!(classified.0[1].classification.confidence, 0.5);

    // Tier 1 sees only `bbb` as below threshold 1.0.
    let response = serde_json::json!({
        "verdicts": [
            {"index": 0, "section": "bug-fixes", "summary": "Fix b", "confidence": 0.7}
        ]
    });
    let provider = MockProvider::returning(response.to_string());
    let audit = AuditSink::new();

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
    assert_eq!(
        updated, 1,
        "only the cache-miss commit should be reclassified"
    );
    assert_eq!(provider.call_count(), 1);

    // Cached classification preserved through the ladder.
    assert_eq!(classified.0[0].classification.summary, "Cached: add login");
}

#[test]
fn persist_only_writes_llm_derived_to_disk() {
    let dir = TempDir::new().unwrap();
    let cache = DiskCache::new(dir.path().to_path_buf());
    let model = "m";

    // Mix of sources.
    let mut a = make_commit("aaa", "x");
    a.classification.source = ClassificationSource::Conventional;
    a.classification.confidence = 1.0;

    let mut b = make_commit("bbb", "y");
    b.classification.source = ClassificationSource::BatchedLlm;
    b.classification.confidence = 0.85;

    let mut c = make_commit("ccc", "z");
    c.classification.source = ClassificationSource::Agentic;
    c.classification.confidence = 0.95;

    let classified = Classified(vec![a, b, c]);
    let written = cache.persist_llm_results(&classified, model);
    assert_eq!(written, 2, "only LLM-derived sources persist");
    assert!(cache.get("aaa", model).is_none()); // Conventional
    assert!(cache.get("bbb", model).is_some()); // BatchedLlm
    assert!(cache.get("ccc", model).is_some()); // Agentic
}

#[test]
fn schema_v1_layout_is_used() {
    let dir = TempDir::new().unwrap();
    let cache = DiskCache::new(dir.path().to_path_buf());
    cache.put(
        "abcdef",
        "model",
        &Classification {
            section: Section::Features,
            summary: "x".into(),
            source: ClassificationSource::BatchedLlm,
            confidence: 0.9,
        },
    );

    // The cache file should appear under the v1 schema dir.
    let v1 = dir.path().join("v1");
    assert!(v1.exists(), "expected v1 schema directory");

    // And `cache.root()` should point at the same place.
    assert_eq!(cache.root().unwrap(), v1);
}

#[test]
fn smoke_test_cache_against_real_repo() {
    // Just exercise the path-resolution helper end-to-end with a temp repo.
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_path_buf();
    run_git(&path, &["init", "--initial-branch=main"]);
    run_git(&path, &["config", "user.name", "ada"]);
    run_git(&path, &["config", "user.email", "ada@x"]);
    run_git(&path, &["config", "commit.gpgsign", "false"]);

    let cache_root = chronikl::cache::default_cache_root(&path);
    // Without CHRONIKL_CACHE_DIR set, default_cache_root returns the
    // OS cache dir (or the repo-local fallback). Either is fine; just
    // check it produces *some* path.
    assert!(!cache_root.as_os_str().is_empty());
}
