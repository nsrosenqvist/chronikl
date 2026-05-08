//! End-to-end test: real git repo → resolve range → log → Tier 0
//! classify → Markdown render. Validates the full deterministic pipeline
//! before any LLM is involved.

use std::path::Path;
use std::process::Command;

use chronikl::git::{self, RangeSpec};
use chronikl::ladder::tier0;
use chronikl::output::{self, RenderOptions};
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
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(full, contents).unwrap();
    }

    fn commit(&self, subject: &str, body: Option<&str>) {
        run(&self.path, &["add", "-A"]);
        let msg = match body {
            Some(b) => format!("{subject}\n\n{b}"),
            None => subject.to_string(),
        };
        run(&self.path, &["commit", "-m", &msg]);
    }

    fn tag(&self, name: &str) {
        run(&self.path, &["tag", name]);
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
async fn end_to_end_renders_grouped_markdown() {
    let repo = Repo::init();

    repo.write("README.md", "v0\n");
    repo.commit("chore: initial", None);
    repo.tag("v0.1.0");

    repo.write("src/login.rs", "pub fn login() {}\n");
    repo.commit("feat(auth): add login flow", None);

    repo.write("src/login.rs", "pub fn login() { println!(\"hi\"); }\n");
    repo.commit("fix(auth): print greeting (#42)", None);

    repo.write("Cargo.lock", "v=1\n");
    repo.commit("chore(deps): bump dependencies", None);

    repo.write(".github/workflows/ci.yml", "name: CI\n");
    repo.commit("Update CI matrix", None); // No conventional prefix; files-only heuristic should classify as CI.

    repo.write("docs/guide.md", "# Guide\n");
    repo.commit("Add user guide", None); // Files-only heuristic → docs.

    repo.write("BUMP", "v2\n");
    repo.commit(
        "feat!: drop legacy API",
        Some("BREAKING CHANGE: /v1 endpoints removed."),
    );

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
    let classified = tier0::classify(&enriched);
    let md = output::render(&classified, &RenderOptions::default());

    // All expected sections present and ordered correctly.
    let i_breaking = md.find("### Breaking Changes").expect("breaking section");
    let i_features = md.find("### Features").expect("features section");
    let i_fixes = md.find("### Bug Fixes").expect("fixes section");
    let i_docs = md.find("### Documentation").expect("docs section");
    let i_ci = md.find("### CI").expect("ci section");
    let i_chore = md.find("### Chore").expect("chore section");

    assert!(i_breaking < i_features);
    assert!(i_features < i_fixes);
    assert!(i_fixes < i_docs);
    assert!(i_docs < i_ci);
    assert!(i_ci < i_chore);

    // Specific bullets and behaviors.
    assert!(md.contains("- drop legacy API"), "rendered: {md}");
    assert!(md.contains("- add login flow"), "rendered: {md}");
    assert!(
        md.contains("- print greeting (#42)"),
        "PR-suffixed bullet should keep the suffix: {md}"
    );
    assert!(md.contains("- bump dependencies"));
    assert!(md.contains("- Update CI matrix"));
    assert!(md.contains("- Add user guide"));
}

#[tokio::test]
async fn empty_range_renders_no_changes_line() {
    let repo = Repo::init();
    repo.write("a", "1\n");
    repo.commit("chore: seed", None);
    repo.tag("v0.1.0");

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
    let classified = tier0::classify(&enriched);
    let md = output::render(&classified, &RenderOptions::default());

    assert!(md.contains("_No changes._"));
}

#[tokio::test]
async fn lockfile_only_commit_is_chore_even_without_conventional_prefix() {
    let repo = Repo::init();
    repo.write("a", "1\n");
    repo.commit("chore: seed", None);
    repo.tag("v0.1.0");

    repo.write("Cargo.lock", "v=2\n");
    repo.commit("Bump deps", None); // No prefix → relies on files-only heuristic.

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
    let classified = tier0::classify(&enriched);
    let md = output::render(&classified, &RenderOptions::default());

    assert!(md.contains("### Chore"));
    assert!(md.contains("- Bump deps"));
    assert!(!md.contains("### Other"));
}

#[tokio::test]
async fn unclassifiable_commit_lands_in_other_with_low_confidence() {
    let repo = Repo::init();
    repo.write("a", "1\n");
    repo.commit("chore: seed", None);
    repo.tag("v0.1.0");

    repo.write("src/main.rs", "fn main() {}\n");
    repo.commit("wip", None);

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
    let classified = tier0::classify(&enriched);

    let other = classified
        .iter()
        .find(|c| c.commit.subject == "wip")
        .unwrap();
    assert_eq!(
        other.classification.section,
        chronikl::models::Section::Other
    );
    assert_eq!(other.classification.confidence, 0.5);
}
