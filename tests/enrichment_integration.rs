//! Enrichment integration tests.
//!
//! - Verifies the platform detector picks the no-op when there's no
//!   GitHub remote.
//! - Manually attaches `PrInfo` to commits and asserts that the Tier 1
//!   prompt builder surfaces PR title/body/labels.
//! - One `#[ignore]`d test fetches a real PR from a public repo.

use std::path::Path;
use std::process::Command;

use chronikl::audit::AuditSink;
use chronikl::enrichment::{NoOpEnricher, PrEnricher, platform};
use chronikl::env::Env;
use chronikl::git::{self, RangeSpec};
use chronikl::ladder::{tier0, tier1};
use chronikl::models::PrInfo;
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

fn make_repo(remote_url: Option<&str>) -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_path_buf();
    run_git(&path, &["init", "--initial-branch=main"]);
    run_git(&path, &["config", "user.name", "Ada"]);
    run_git(&path, &["config", "user.email", "ada@x"]);
    run_git(&path, &["config", "commit.gpgsign", "false"]);
    if let Some(url) = remote_url {
        run_git(&path, &["remote", "add", "origin", url]);
    }
    std::fs::write(path.join("README.md"), "v0\n").unwrap();
    run_git(&path, &["add", "."]);
    run_git(&path, &["commit", "-m", "chore: seed"]);
    run_git(&path, &["tag", "v0.1.0"]);
    std::fs::write(path.join("a.rs"), "fn a() {}\n").unwrap();
    run_git(&path, &["add", "."]);
    run_git(&path, &["commit", "-m", "wip"]);
    (dir, path)
}

#[tokio::test]
async fn detector_picks_no_op_when_no_github_remote() {
    let (_dir, repo) = make_repo(Some("https://gitlab.com/foo/bar.git"));
    let env = Env::mock(Vec::<(&str, &str)>::new());
    let (e, info) = platform::detect(&repo, &env, false).await.unwrap();
    assert_eq!(e.name(), "no-op");
    assert!(info.contains("no GitHub remote"));
}

#[tokio::test]
async fn detector_picks_github_when_origin_is_github() {
    let (_dir, repo) = make_repo(Some("https://github.com/foo/bar.git"));
    let env = Env::mock([("GITHUB_TOKEN", "ghp_test")]);
    let (e, info) = platform::detect(&repo, &env, false).await.unwrap();
    assert_eq!(e.name(), "github");
    assert!(info.contains("foo/bar"));
    assert!(info.contains("authenticated"));
}

#[tokio::test]
async fn detector_picks_github_anonymous_when_no_token() {
    let (_dir, repo) = make_repo(Some("https://github.com/foo/bar.git"));
    let env = Env::mock(Vec::<(&str, &str)>::new());
    let (_e, info) = platform::detect(&repo, &env, false).await.unwrap();
    assert!(info.contains("anonymous"));
}

#[tokio::test]
async fn disabled_returns_no_op_even_on_github() {
    let (_dir, repo) = make_repo(Some("https://github.com/foo/bar.git"));
    let env = Env::mock([("GITHUB_TOKEN", "ghp_test")]);
    let (e, info) = platform::detect(&repo, &env, true).await.unwrap();
    assert_eq!(e.name(), "no-op");
    assert!(info.contains("disabled"));
}

#[tokio::test]
async fn no_op_enricher_returns_zero() {
    let (_dir, repo) = make_repo(None);
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
    let mut enriched = chronikl::models::EnrichedCommit::from_commits(commits);
    let outcome = NoOpEnricher.enrich(&mut enriched).await.unwrap();
    assert_eq!(outcome.enriched, 0);
    assert_eq!(outcome.failed, 0);
    assert!(enriched.iter().all(|c| c.pr.is_none()));
}

#[tokio::test]
async fn pr_data_surfaces_in_tier1_prompt() {
    let (_dir, repo) = make_repo(None);
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
    let mut enriched = chronikl::models::EnrichedCommit::from_commits(commits);
    // Manually attach PR data to simulate a successful enrichment pass.
    enriched[0].pr = Some(PrInfo {
        number: 42,
        title: "Add a new login flow".into(),
        body: "Adds /v2/login that supports OIDC.".into(),
        labels: vec!["enhancement".into(), "auth".into()],
        author: Some("ada".into()),
        merged_at: None,
        url: None,
    });

    let mut classified = tier0::classify(&enriched);
    let resp = serde_json::json!({
        "verdicts": [
            {"index": 0, "section": "features", "summary": "Add login flow", "confidence": 0.9}
        ]
    });
    let provider = MockProvider::returning(resp.to_string());
    let audit = AuditSink::new();

    tier1::classify(
        &mut classified,
        &provider as &dyn NotesProvider,
        &audit,
        50,
        1.0,
        tier1::default_system_prompt(),
    )
    .await
    .unwrap();

    let calls = provider.captured_calls();
    let prompt = &calls[0].user_prompt;
    // PR fields are XML-fenced as part of the prompt-injection
    // defense — assert the fenced form rather than ad-hoc labels.
    assert!(
        prompt.contains("<pr_title>\nAdd a new login flow\n</pr_title>"),
        "PR title should appear fenced in the Tier 1 prompt: {prompt}"
    );
    assert!(
        prompt.contains("<pr_body>\nAdds /v2/login that supports OIDC.\n</pr_body>"),
        "PR body should appear fenced in the Tier 1 prompt: {prompt}"
    );
    assert!(
        prompt.contains("<pr_labels>\nenhancement, auth\n</pr_labels>"),
        "PR labels should appear fenced in the Tier 1 prompt: {prompt}"
    );
}

#[tokio::test]
#[ignore = "hits real GitHub API; rate-limited unless GITHUB_TOKEN is set"]
async fn anonymous_github_fetches_real_pr_for_known_commit() {
    use chronikl::enrichment::GitHubEnricher;
    use chronikl::enrichment::remote::GitHubRepo;

    // octocrab-rs has stable PRs we can pin to. We use any PR's merge
    // commit SHA from the octocrab repo. PR #1 is far enough back that
    // it's stable.
    let repo = GitHubRepo {
        owner: "XAMPPRocky".into(),
        repo: "octocrab".into(),
    };
    let token = std::env::var("GITHUB_TOKEN").ok();
    let enricher = GitHubEnricher::new(repo, token).unwrap();

    // PR #1 merge commit on octocrab — known stable.
    let mut commits = vec![chronikl::models::EnrichedCommit::from_commit(
        chronikl::models::Commit {
            sha: "08f6e62a8e74ec2ba94d6da92127c27d7f57ca50".into(),
            short_sha: "08f6e62".into(),
            author_name: "x".into(),
            author_email: "x@x".into(),
            author_date: "2020-01-01T00:00:00+00:00".into(),
            parents: vec!["p".into()],
            subject: "test".into(),
            body: String::new(),
            files: vec![],
            pr_id: None,
            conventional: None,
            breaking: false,
        },
    )];
    let _ = enricher.enrich(&mut commits).await.unwrap();
    // We don't assert the count strictly because the SHA may not resolve
    // to a PR (e.g. if it was rebased). The point is the call succeeded
    // without panicking and the pass is best-effort.
    eprintln!("anonymous fetch returned: pr={:?}", commits[0].pr);
}
