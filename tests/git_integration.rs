//! End-to-end tests for the git driver against a real, throwaway git repo.
//!
//! These exercise the actual `git` subprocess path that unit tests can't
//! reach. Each test creates a temp repo, scripts a commit history with
//! known characteristics, then asserts the parsed [`Commit`] objects.

use std::path::Path;
use std::process::Command;

use chronikl::git::{self, RangeSpec};
use chronikl::models::{Commit, MergeStyle};
use tempfile::TempDir;

struct Repo {
    _dir: TempDir,
    path: std::path::PathBuf,
}

impl Repo {
    fn init() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().to_path_buf();
        run(&path, &["init", "--initial-branch=main"]);
        run(&path, &["config", "user.name", "Ada Lovelace"]);
        run(&path, &["config", "user.email", "ada@example.com"]);
        run(&path, &["config", "commit.gpgsign", "false"]);
        Self { _dir: dir, path }
    }

    fn write(&self, rel: &str, contents: &str) {
        let full = self.path.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("mkdirs");
        }
        std::fs::write(full, contents).expect("write");
    }

    fn commit(&self, subject: &str, body: Option<&str>) {
        run(&self.path, &["add", "-A"]);
        let msg = match body {
            Some(b) => format!("{subject}\n\n{b}"),
            None => subject.to_string(),
        };
        run(&self.path, &["commit", "--allow-empty", "-m", &msg]);
    }

    fn tag(&self, name: &str) {
        run(&self.path, &["tag", name]);
    }

    fn merge(&self, branch: &str, subject: &str) {
        run(&self.path, &["merge", "--no-ff", "-m", subject, branch]);
    }

    fn checkout(&self, args: &[&str]) {
        let mut full = vec!["checkout"];
        full.extend(args);
        run(&self.path, &full);
    }
}

fn run(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("spawn git");
    if !out.status.success() {
        panic!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[tokio::test]
async fn parses_conventional_commits_with_files_and_pr_id() {
    let repo = Repo::init();

    repo.write("README.md", "init\n");
    repo.commit("chore: initial commit", None);
    repo.tag("v0.1.0");

    repo.write("src/lib.rs", "pub fn hello() {}\n");
    repo.commit("feat(api): add hello endpoint", None);

    repo.write("src/lib.rs", "pub fn hello() { println!(\"hi\"); }\n");
    repo.commit(
        "fix(api): print greeting (#42)",
        Some("Was a no-op before."),
    );

    repo.write("CHANGELOG.md", "v2!\n");
    repo.commit(
        "feat!: drop old API",
        Some("BREAKING CHANGE: /v1 is removed."),
    );

    let range = git::resolve_range(
        &repo.path,
        &RangeSpec::Explicit {
            from: Some("v0.1.0".into()),
            to: "HEAD".into(),
        },
    )
    .await
    .unwrap();

    let commits = git::log(&repo.path, &range).await.unwrap();
    assert_eq!(commits.len(), 3, "expected 3 commits past v0.1.0");

    // git log is reverse-chronological — newest first.
    let breaking = &commits[0];
    assert!(
        breaking.breaking,
        "BREAKING CHANGE: footer should be detected"
    );
    assert_eq!(breaking.conventional.as_ref().unwrap().kind, "feat");
    assert!(
        breaking.conventional.as_ref().unwrap().breaking,
        "conventional.breaking should be true via the `!` marker"
    );

    let fix = &commits[1];
    assert_eq!(fix.pr_id, Some(42));
    assert_eq!(fix.conventional.as_ref().unwrap().kind, "fix");
    assert_eq!(
        fix.conventional.as_ref().unwrap().scope.as_deref(),
        Some("api")
    );

    let feat = &commits[2];
    assert_eq!(feat.conventional.as_ref().unwrap().kind, "feat");
    assert!(feat.files.iter().any(|f| f == "src/lib.rs"));
}

#[tokio::test]
async fn auto_range_picks_previous_tag() {
    let repo = Repo::init();
    repo.write("a", "1\n");
    repo.commit("chore: a", None);
    repo.tag("v0.1.0");
    repo.write("b", "1\n");
    repo.commit("feat: b", None);
    repo.tag("v0.2.0");
    repo.write("c", "1\n");
    repo.commit("fix: c", None);

    let resolved = git::resolve_range(&repo.path, &RangeSpec::Auto)
        .await
        .unwrap();

    // HEAD is not tagged → from is the latest tag.
    assert_eq!(resolved.from.as_deref(), Some("v0.2.0"));

    let commits = git::log(&repo.path, &resolved).await.unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].subject, "fix: c");
}

#[tokio::test]
async fn auto_range_when_head_is_tagged_uses_previous_tag() {
    let repo = Repo::init();
    repo.write("a", "1\n");
    repo.commit("chore: a", None);
    repo.tag("v0.1.0");
    repo.write("b", "1\n");
    repo.commit("feat: b", None);
    repo.tag("v0.2.0");

    let resolved = git::resolve_range(&repo.path, &RangeSpec::Auto)
        .await
        .unwrap();

    // HEAD is at v0.2.0 → Auto walks back to v0.1.0.
    assert_eq!(resolved.from.as_deref(), Some("v0.1.0"));
    let commits = git::log(&repo.path, &resolved).await.unwrap();
    assert_eq!(commits.len(), 1);
    assert!(commits[0].subject.contains("feat: b"));
}

#[tokio::test]
async fn since_last_tag_includes_head_when_tagged() {
    let repo = Repo::init();
    repo.write("a", "1\n");
    repo.commit("chore: a", None);
    repo.tag("v0.1.0");
    repo.write("b", "1\n");
    repo.commit("feat: b", None);
    repo.tag("v0.2.0");

    let resolved = git::resolve_range(&repo.path, &RangeSpec::SinceLastTag)
        .await
        .unwrap();

    // SinceLastTag uses the latest tag as `from` even when HEAD is at it
    // (so range is empty here, by design — it's the user-facing
    // `--since-last-tag` semantic).
    assert_eq!(resolved.from.as_deref(), Some("v0.2.0"));
}

#[tokio::test]
async fn detects_squash_merge_style() {
    let repo = Repo::init();
    repo.write("seed", "1\n");
    repo.commit("chore: seed", None);

    for (i, subject) in [
        "Add login flow (#10)",
        "Fix race in cache (#11)",
        "Bump deps (#12)",
        "Refactor handler (#13)",
        "Add metrics (#14)",
    ]
    .iter()
    .enumerate()
    {
        repo.write(&format!("file_{i}.txt"), "x\n");
        repo.commit(subject, None);
    }

    let range = git::resolve_range(
        &repo.path,
        &RangeSpec::Explicit {
            from: None,
            to: "HEAD".into(),
        },
    )
    .await
    .unwrap();
    let commits = git::log(&repo.path, &range).await.unwrap();
    let style = chronikl::git::merge_style::detect(&commits);
    assert_eq!(style, MergeStyle::Squash);
}

#[tokio::test]
async fn detects_merge_commit_style() {
    let repo = Repo::init();
    repo.write("seed", "1\n");
    repo.commit("chore: seed", None);

    // Branch out, commit, merge non-FF — twice.
    for i in 0..3 {
        let branch = format!("feature/{i}");
        run(&repo.path, &["checkout", "-b", &branch]);
        repo.write(&format!("f{i}.txt"), "x\n");
        repo.commit(&format!("feat: thing {i}"), None);
        repo.checkout(&["main"]);
        repo.merge(&branch, &format!("Merge pull request #{i} from {branch}"));
    }

    let range = git::resolve_range(
        &repo.path,
        &RangeSpec::Explicit {
            from: None,
            to: "HEAD".into(),
        },
    )
    .await
    .unwrap();
    let commits = git::log(&repo.path, &range).await.unwrap();
    let style = chronikl::git::merge_style::detect(&commits);
    assert!(
        matches!(style, MergeStyle::Merge | MergeStyle::Mixed),
        "expected Merge or Mixed, got {style}"
    );
}

#[tokio::test]
async fn merge_commits_have_multiple_parents() {
    let repo = Repo::init();
    repo.write("seed", "1\n");
    repo.commit("chore: seed", None);
    run(&repo.path, &["checkout", "-b", "feature"]);
    repo.write("a", "x\n");
    repo.commit("feat: a", None);
    repo.checkout(&["main"]);
    repo.merge("feature", "Merge feature");

    let range = git::resolve_range(
        &repo.path,
        &RangeSpec::Explicit {
            from: None,
            to: "HEAD".into(),
        },
    )
    .await
    .unwrap();
    let commits = git::log(&repo.path, &range).await.unwrap();

    let merge_commit = commits
        .iter()
        .find(|c: &&Commit| c.subject == "Merge feature")
        .expect("merge commit");
    assert!(merge_commit.is_merge());
    assert_eq!(merge_commit.parents.len(), 2);
}

#[tokio::test]
async fn handles_multiline_body_with_separators_in_text() {
    let repo = Repo::init();
    repo.write("a", "1\n");
    // Body deliberately contains text that could confuse a naive parser.
    repo.commit(
        "feat: tricky body",
        Some(
            "First line.\nSecond line: with a colon.\n\nA paragraph break.\n\
             Another line.\n\n - bullet\n - another\n",
        ),
    );

    let range = git::resolve_range(
        &repo.path,
        &RangeSpec::Explicit {
            from: None,
            to: "HEAD".into(),
        },
    )
    .await
    .unwrap();
    let commits = git::log(&repo.path, &range).await.unwrap();

    assert_eq!(commits.len(), 1);
    let c = &commits[0];
    assert_eq!(c.subject, "feat: tricky body");
    assert!(c.body.contains("Second line: with a colon."));
    assert!(c.body.contains("- bullet"));
}
