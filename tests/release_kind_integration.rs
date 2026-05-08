//! Integration tests for release-kind detection: real git repo +
//! `git::detect_release_kind`.

use std::path::Path;
use std::process::Command;

use chronikl::git;
use chronikl::models::{ReleaseKind, VersionBump, VersionScheme};
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

fn make_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    run_git(dir.path(), &["init", "--initial-branch=main"]);
    run_git(dir.path(), &["config", "user.name", "Ada"]);
    run_git(dir.path(), &["config", "user.email", "ada@x"]);
    run_git(dir.path(), &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.path().join("a"), "1\n").unwrap();
    run_git(dir.path(), &["add", "."]);
    run_git(dir.path(), &["commit", "-m", "chore: seed"]);
    dir
}

#[tokio::test]
async fn detects_stable_from_explicit_ref() {
    let dir = make_repo();
    let kind = git::detect_release_kind(dir.path(), "v1.2.3")
        .await
        .unwrap();
    assert_eq!(kind, ReleaseKind::Stable);
}

#[tokio::test]
async fn detects_prerelease_from_explicit_ref() {
    let dir = make_repo();
    let kind = git::detect_release_kind(dir.path(), "v1.0.0-rc.1")
        .await
        .unwrap();
    assert_eq!(
        kind,
        ReleaseKind::Prerelease {
            label: "rc.1".into()
        }
    );
}

#[tokio::test]
async fn detects_prerelease_from_tag_at_head() {
    let dir = make_repo();
    run_git(dir.path(), &["tag", "v0.5.0-alpha"]);
    let kind = git::detect_release_kind(dir.path(), "HEAD").await.unwrap();
    assert_eq!(
        kind,
        ReleaseKind::Prerelease {
            label: "alpha".into()
        }
    );
}

#[tokio::test]
async fn detects_stable_from_tag_at_head() {
    let dir = make_repo();
    run_git(dir.path(), &["tag", "v1.0.0"]);
    let kind = git::detect_release_kind(dir.path(), "HEAD").await.unwrap();
    assert_eq!(kind, ReleaseKind::Stable);
}

#[tokio::test]
async fn untagged_head_returns_untagged() {
    let dir = make_repo();
    let kind = git::detect_release_kind(dir.path(), "HEAD").await.unwrap();
    assert_eq!(kind, ReleaseKind::Untagged);
}

// ── VersionBump detection ──────────────────────────────────────────

#[tokio::test]
async fn bump_initial_when_no_from_supplied() {
    let dir = make_repo();
    let (bump, _scheme) = git::detect_version_bump(dir.path(), None, "v1.0.0")
        .await
        .unwrap();
    assert_eq!(bump, VersionBump::Initial);
}

#[tokio::test]
async fn bump_major_from_explicit_refs() {
    let dir = make_repo();
    let (bump, _scheme) = git::detect_version_bump(dir.path(), Some("v1.5.3"), "v2.0.0")
        .await
        .unwrap();
    assert_eq!(bump, VersionBump::Major);
}

#[tokio::test]
async fn bump_minor_from_explicit_refs() {
    let dir = make_repo();
    let (bump, _scheme) = git::detect_version_bump(dir.path(), Some("v1.5.3"), "v1.6.0")
        .await
        .unwrap();
    assert_eq!(bump, VersionBump::Minor);
}

#[tokio::test]
async fn bump_patch_from_explicit_refs() {
    let dir = make_repo();
    let (bump, _scheme) = git::detect_version_bump(dir.path(), Some("v1.5.3"), "v1.5.4")
        .await
        .unwrap();
    assert_eq!(bump, VersionBump::Patch);
}

#[tokio::test]
async fn bump_resolves_tag_at_head() {
    let dir = make_repo();
    run_git(dir.path(), &["tag", "v1.0.0"]);
    let (bump, _scheme) = git::detect_version_bump(dir.path(), Some("v0.5.0"), "HEAD")
        .await
        .unwrap();
    assert_eq!(bump, VersionBump::Major);
}

#[tokio::test]
async fn bump_unknown_for_non_semver_to() {
    let dir = make_repo();
    let (bump, _scheme) = git::detect_version_bump(dir.path(), Some("v1.0.0"), "HEAD")
        .await
        .unwrap();
    assert_eq!(bump, VersionBump::Unknown);
}

#[tokio::test]
async fn bump_for_major_prerelease() {
    let dir = make_repo();
    let (bump, _scheme) = git::detect_version_bump(dir.path(), Some("v1.5.0"), "v2.0.0-rc.1")
        .await
        .unwrap();
    assert_eq!(bump, VersionBump::Major);
}

// ── CalVer ────────────────────────────────────────────────────────

#[tokio::test]
async fn calver_padded_year_month_recognised() {
    let dir = make_repo();
    let kind = git::detect_release_kind(dir.path(), "v2024.05.08")
        .await
        .unwrap();
    assert_eq!(kind, ReleaseKind::Stable);

    let (bump, scheme) = git::detect_version_bump(dir.path(), Some("v2024.05.08"), "v2024.06.01")
        .await
        .unwrap();
    assert_eq!(bump, VersionBump::Minor);
    assert_eq!(scheme, Some(VersionScheme::Calver));
}

#[tokio::test]
async fn calver_year_change_is_major() {
    let dir = make_repo();
    let (bump, scheme) = git::detect_version_bump(dir.path(), Some("v2024.12.01"), "v2025.01.05")
        .await
        .unwrap();
    assert_eq!(bump, VersionBump::Major);
    assert_eq!(scheme, Some(VersionScheme::Calver));
}

#[tokio::test]
async fn calver_with_prerelease_suffix() {
    let dir = make_repo();
    let kind = git::detect_release_kind(dir.path(), "v2024.05.08-rc1")
        .await
        .unwrap();
    assert!(matches!(kind, ReleaseKind::Prerelease { .. }));
}

#[tokio::test]
async fn cross_scheme_returns_unknown_bump() {
    let dir = make_repo();
    // semver from-ref vs CalVer to-ref → Unknown.
    let (bump, _scheme) = git::detect_version_bump(dir.path(), Some("v1.5.0"), "v2024.05.08")
        .await
        .unwrap();
    assert_eq!(bump, VersionBump::Unknown);
}

#[tokio::test]
async fn prefers_first_recognisable_tag_when_multiple_point_at_ref() {
    let dir = make_repo();
    // Multiple tags point at HEAD: a non-semver one and a semver prerelease.
    run_git(dir.path(), &["tag", "release-candidate"]);
    run_git(dir.path(), &["tag", "v2.0.0-beta.2"]);
    let kind = git::detect_release_kind(dir.path(), "HEAD").await.unwrap();
    // The non-semver tag is skipped; the semver one wins.
    assert!(matches!(kind, ReleaseKind::Prerelease { .. }));
}
