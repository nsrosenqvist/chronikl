//! End-to-end CLI tests for rendering controls, the `simple` voice,
//! voice frontmatter, "nothing user-facing" lines and `--strict`. They run the real binary against a throwaway repo
//! with a scrubbed environment, and never reach an LLM: runs either use
//! `--no-llm` or have no usable provider.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn run_git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit(repo: &Path, file: &str, message: &str) {
    std::fs::write(repo.join(file), message).unwrap();
    run_git(repo, &["add", "."]);
    run_git(repo, &["commit", "-m", message]);
}

/// A repo with a `v0.1.0` tag and five Conventional commits after it.
fn make_repo() -> TempDir {
    make_repo_with(&[
        ("search", "feat: add search"),
        ("crash", "fix: crash on start"),
        ("tests", "test: add sign-in tests"),
        ("README.md", "docs: describe setup"),
        ("ci", "ci: cache dependencies"),
    ])
}

/// A repo with a `v0.1.0` tag and the given `(file, message)` commits
/// after it.
fn make_repo_with(commits: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    run_git(repo, &["init", "--initial-branch=main"]);
    run_git(repo, &["config", "user.name", "Ada"]);
    run_git(repo, &["config", "user.email", "ada@x"]);
    run_git(repo, &["config", "commit.gpgsign", "false"]);
    run_git(repo, &["config", "tag.gpgsign", "false"]);
    commit(repo, "seed", "chore: seed");
    run_git(repo, &["tag", "v0.1.0"]);
    for (file, message) in commits {
        commit(repo, file, message);
    }
    dir
}

/// Run chronikl in `repo` with only the given environment variables
/// (plus what git needs).
fn chronikl(repo: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let home = repo.join(".home");
    std::fs::create_dir_all(&home).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_chronikl"));
    cmd.current_dir(repo)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("CHRONIKL_CACHE_DIR", home.join("cache"))
        .args(["--from", "v0.1.0", "--to", "HEAD", "--no-pr-enrichment"])
        .args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn terse_default_keeps_every_section_with_headings() {
    let dir = make_repo();
    let out = chronikl(dir.path(), &["--no-llm"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    for heading in [
        "## What's Changed",
        "### Features",
        "### Bug Fixes",
        "### Documentation",
        "### Tests",
        "### CI",
    ] {
        assert!(text.contains(heading), "{heading} missing: {text}");
    }
}

#[test]
fn simple_voice_renders_flat_user_visible_list() {
    let dir = make_repo();
    let out = chronikl(dir.path(), &["--no-llm", "--voice", "simple"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "- add search\n- crash on start\n");
}

#[test]
fn flags_win_over_the_voice_frontmatter() {
    let dir = make_repo();
    let out = chronikl(
        dir.path(),
        &[
            "--no-llm",
            "--voice",
            "simple",
            "--headings",
            "--include-sections",
            "features,tests",
        ],
        &[],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Features\n\n- add search"), "{text}");
    assert!(text.contains("Tests\n\n- add sign-in tests"), "{text}");
    assert!(!text.contains("crash on start"), "{text}");
}

#[test]
fn custom_voice_frontmatter_shapes_the_output() {
    let dir = make_repo();
    std::fs::write(
        dir.path().join("store-voice.md"),
        "---\nheadings: false\nfooter: false\nexclude_sections: [tests, docs, ci, fixes]\n---\nWrite for testers.\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".chronikl.toml"),
        "[voice]\npath = \"store-voice.md\"\n",
    )
    .unwrap();
    let out = chronikl(dir.path(), &["--no-llm"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "- add search\n");
}

#[test]
fn toml_render_beats_the_voice_frontmatter() {
    let dir = make_repo();
    std::fs::write(
        dir.path().join(".chronikl.toml"),
        "[voice]\nprofile = \"simple\"\n[render]\nexclude_sections = [\"fixes\"]\n",
    )
    .unwrap();
    let out = chronikl(dir.path(), &["--no-llm"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    // `fixes` replaces the voice's exclusions, so tests/docs/ci are back.
    let text = stdout(&out);
    assert!(text.contains("- add sign-in tests"), "{text}");
    assert!(!text.contains("crash on start"), "{text}");
}

/// Run the `simple` voice with `--no-llm` over `commits` plus `args`.
fn simple_run(commits: &[(&str, &str)], args: &[&str]) -> (Output, serde_json::Value) {
    let dir = make_repo_with(commits);
    let audit = dir.path().join(".home").join("audit.json");
    std::fs::create_dir_all(dir.path().join(".home")).unwrap();
    let mut all = vec![
        "--no-llm",
        "--strict",
        "--voice",
        "simple",
        "--audit-log",
        audit.to_str().unwrap(),
    ];
    all.extend_from_slice(args);
    let out = chronikl(dir.path(), &all, &[]);
    let json = serde_json::from_str(&std::fs::read_to_string(&audit).unwrap()).unwrap();
    (out, json)
}

#[test]
fn docs_and_tests_only_range_gets_the_other_line() {
    let (out, audit) = simple_run(
        &[
            ("README.md", "docs: describe setup"),
            ("t", "test: cover it"),
        ],
        &[],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Small improvements behind the scenes.\n");
    assert_eq!(audit["notes_source"]["kind"], "nothing_user_facing");
    assert_eq!(audit["notes_source"]["fallback"], "other");
}

#[test]
fn internal_fixes_only_range_gets_the_fixes_line() {
    let (out, _) = simple_run(
        &[("ci", "fix(ci): retry flaky job"), ("b", "build: pin node")],
        &["--exclude-sections", "fixes,build"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Bug fixes and stability improvements.\n");
}

#[test]
fn performance_only_range_gets_the_performance_line() {
    let (out, _) = simple_run(
        &[("p", "perf: cache tile lookups")],
        &["--exclude-sections", "perf"],
    );
    assert_eq!(stdout(&out), "Faster and smoother in places.\n");
}

#[test]
fn mixed_range_shows_only_the_user_facing_items() {
    let (out, audit) = simple_run(
        &[("s", "feat: add search"), ("t", "test: cover search")],
        &[],
    );
    assert_eq!(stdout(&out), "- add search\n");
    assert_eq!(audit["notes_source"]["kind"], "deterministic");
}

#[test]
fn empty_range_gets_the_other_line() {
    let dir = make_repo_with(&[]);
    let out = chronikl(dir.path(), &["--no-llm", "--voice", "simple"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Small improvements behind the scenes.\n");
}

#[test]
fn fallback_lines_can_be_overridden() {
    let (out, _) = simple_run(
        &[("README.md", "docs: describe setup")],
        &["--fallback-line", "Polish and fixes."],
    );
    assert_eq!(stdout(&out), "Polish and fixes.\n");
    let (out, _) = simple_run(
        &[("p", "perf: faster")],
        &[
            "--exclude-sections",
            "perf",
            "--fallback-line",
            "Polish.",
            "--fallback-performance",
            "Snappier.",
        ],
    );
    assert_eq!(stdout(&out), "Snappier.\n");
}

#[test]
fn unreachable_model_writes_the_fallback_line_not_commit_subjects() {
    let dir = make_repo();
    let out = chronikl(
        dir.path(),
        &["--voice", "simple"],
        &[
            ("CHRONIKL_PROVIDER", "not-a-provider"),
            ("CHRONIKL_MODEL", "m"),
            ("CHRONIKL_API_KEY", "k"),
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Bug fixes and stability improvements.\n");
}

#[test]
fn render_settings_come_from_env_and_toml() {
    let dir = make_repo();
    std::fs::write(
        dir.path().join(".chronikl.toml"),
        "[render]\nheadings = false\nexclude_sections = [\"docs\", \"ci\"]\n",
    )
    .unwrap();
    let out = chronikl(dir.path(), &["--no-llm"], &[("CHRONIKL_MAX_BULLETS", "2")]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "- add search\n- crash on start\n");
}

#[test]
fn max_chars_cuts_at_whole_bullets() {
    let dir = make_repo();
    let out = chronikl(
        dir.path(),
        &["--no-llm", "--headings=false", "--max-chars", "20"],
        &[],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "- add search\n");
}

#[test]
fn fallback_without_strict_still_succeeds() {
    let dir = make_repo();
    let audit = dir.path().join(".home").join("audit.json");
    let out = chronikl(dir.path(), &["--audit-log", audit.to_str().unwrap()], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("### Features"));
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&audit).unwrap()).unwrap();
    assert_eq!(json["notes_source"]["kind"], "fallback");
}

#[test]
fn strict_fails_when_no_provider_is_configured() {
    let dir = make_repo();
    let out = chronikl(dir.path(), &["--strict"], &[]);
    assert!(!out.status.success());
    assert_eq!(stdout(&out), "", "strict failure must write no notes");
    assert!(stderr(&out).contains("strict mode"), "{}", stderr(&out));
}

#[test]
fn strict_fails_when_the_provider_is_unusable_and_writes_the_audit() {
    let dir = make_repo();
    let audit = dir.path().join(".home").join("audit.json");
    let notes = dir.path().join(".home").join("notes.md");
    let out = chronikl(
        dir.path(),
        &[
            "--audit-log",
            audit.to_str().unwrap(),
            "--output",
            notes.to_str().unwrap(),
        ],
        &[
            ("CHRONIKL_STRICT", "true"),
            ("CHRONIKL_PROVIDER", "not-a-provider"),
            ("CHRONIKL_MODEL", "m"),
            ("CHRONIKL_API_KEY", "k"),
        ],
    );
    assert!(!out.status.success());
    assert!(!notes.exists(), "strict failure must not write --output");
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&audit).unwrap()).unwrap();
    assert_eq!(json["notes_source"]["kind"], "fallback");
    assert!(
        json["notes_source"]["reason"]
            .as_str()
            .unwrap()
            .contains("provider not usable")
    );
}

#[test]
fn strict_is_satisfied_by_no_llm() {
    let dir = make_repo();
    let out = chronikl(dir.path(), &["--strict", "--no-llm"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("add search"));
}

#[test]
fn invalid_render_values_are_reported() {
    let dir = make_repo();
    let out = chronikl(
        dir.path(),
        &["--no-llm", "--exclude-sections", "screenshots"],
        &[],
    );
    assert!(!out.status.success());
    assert!(stderr(&out).contains("unknown section"), "{}", stderr(&out));
}
