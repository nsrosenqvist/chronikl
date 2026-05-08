//! Identify the forge backing a git remote and produce forge-specific
//! URLs (compare links, commits views, etc.).
//!
//! A [`Forge`] is the strategy interface — concrete implementations
//! exist for GitHub, GitLab, Bitbucket Cloud, and Gitea/Forgejo. The
//! [`detect_forge`] helper inspects `git remote get-url origin` and
//! returns the matching strategy, or `None` for unrecognized hosts.
//!
//! Forge identification is purely URL-based and uses no API calls, so
//! it can run in any environment (offline, no token, non-CI).

use std::fmt::Debug;
use std::path::Path;

use tokio::process::Command;

/// Strategy interface for forge-specific URL generation. Each
/// implementation knows its compare-URL convention (which differ in
/// significant ways — Bitbucket reverses operands, GitLab uses a
/// `/-/` prefix, etc.).
pub trait Forge: Debug + Send + Sync {
    /// Short identifier used in logs and the audit (`"github"`,
    /// `"gitlab"`, `"bitbucket"`, `"gitea"`).
    fn name(&self) -> &'static str;

    /// URL pointing readers at the commits in this release. With both
    /// bounds present, returns the forge's compare/diff page; with
    /// `from = None` (initial release), falls back to a tag/commits
    /// view so readers still have somewhere to click.
    fn compare_url(&self, from: Option<&str>, to: &str) -> String;
}

// ── GitHub ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRepo {
    pub owner: String,
    pub repo: String,
}

impl Forge for GitHubRepo {
    fn name(&self) -> &'static str {
        "github"
    }

    fn compare_url(&self, from: Option<&str>, to: &str) -> String {
        match from {
            Some(f) => format!(
                "https://github.com/{}/{}/compare/{f}...{to}",
                self.owner, self.repo
            ),
            None => format!(
                "https://github.com/{}/{}/commits/{to}",
                self.owner, self.repo
            ),
        }
    }
}

// ── GitLab ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitLabRepo {
    /// `gitlab.com` or a self-hosted instance hostname.
    pub host: String,
    /// Full path after the host. May contain nested group segments
    /// (e.g. `groupA/groupB/myrepo`) — GitLab is the one big forge that
    /// supports arbitrary nesting.
    pub path: String,
}

impl Forge for GitLabRepo {
    fn name(&self) -> &'static str {
        "gitlab"
    }

    fn compare_url(&self, from: Option<&str>, to: &str) -> String {
        match from {
            Some(f) => format!("https://{}/{}/-/compare/{f}...{to}", self.host, self.path),
            None => format!("https://{}/{}/-/commits/{to}", self.host, self.path),
        }
    }
}

// ── Bitbucket Cloud ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitbucketRepo {
    pub owner: String,
    pub repo: String,
}

impl Forge for BitbucketRepo {
    fn name(&self) -> &'static str {
        "bitbucket"
    }

    fn compare_url(&self, from: Option<&str>, to: &str) -> String {
        match from {
            // Bitbucket Cloud's compare URL uses TWO dots and reverses
            // the operand order (target..source). This is a long-
            // standing inconsistency with the rest of the ecosystem;
            // we test for it explicitly.
            Some(f) => format!(
                "https://bitbucket.org/{}/{}/branches/compare/{to}..{f}",
                self.owner, self.repo
            ),
            None => format!(
                "https://bitbucket.org/{}/{}/commits/tag/{to}",
                self.owner, self.repo
            ),
        }
    }
}

// ── Gitea / Forgejo / Codeberg ──────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GiteaRepo {
    /// `codeberg.org` or a self-hosted Gitea/Forgejo instance.
    pub host: String,
    pub owner: String,
    pub repo: String,
}

impl Forge for GiteaRepo {
    fn name(&self) -> &'static str {
        "gitea"
    }

    fn compare_url(&self, from: Option<&str>, to: &str) -> String {
        match from {
            Some(f) => format!(
                "https://{}/{}/{}/compare/{f}...{to}",
                self.host, self.owner, self.repo
            ),
            // Gitea exposes per-tag commit lists at `commits/tag/<tag>`.
            None => format!(
                "https://{}/{}/{}/commits/tag/{to}",
                self.host, self.owner, self.repo
            ),
        }
    }
}

/// Strip the protocol/auth prefix from a remote URL, returning
/// `Some((host, path))` when recognized. Handles all the common forms
/// (`https://`, `http://`, `git@host:path`, `ssh://git@host/path`,
/// `git://host/path`). The returned `path` still has its `.git`
/// suffix and any trailing slash; callers normalize.
fn split_host_and_path(url: &str) -> Option<(&str, &str)> {
    let trimmed = url.trim();
    // SCP-style: `git@host:path` (no `//`).
    if let Some(rest) = trimmed.strip_prefix("git@")
        && let Some((host, path)) = rest.split_once(':')
    {
        return Some((host, path));
    }
    // URL-style with explicit scheme.
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .or_else(|| trimmed.strip_prefix("ssh://git@"))
        .or_else(|| trimmed.strip_prefix("ssh://"))
        .or_else(|| trimmed.strip_prefix("git://"))?;
    let (host, path) = rest.split_once('/')?;
    Some((host, path))
}

fn normalize_path(path: &str) -> &str {
    path.trim_end_matches(".git").trim_end_matches('/')
}

/// Parse a github.com URL into a [`GitHubRepo`].
pub fn parse_github_remote(url: &str) -> Option<GitHubRepo> {
    let (host, path) = split_host_and_path(url)?;
    if !host.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let path = normalize_path(path);
    let (owner, repo) = path.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some(GitHubRepo {
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

/// Parse a GitLab URL (gitlab.com or self-hosted with `gitlab` in the
/// hostname) into a [`GitLabRepo`]. The `path` may contain nested
/// group segments, which GitLab supports natively.
pub fn parse_gitlab_remote(url: &str) -> Option<GitLabRepo> {
    let (host, path) = split_host_and_path(url)?;
    let host_lower = host.to_ascii_lowercase();
    if host_lower != "gitlab.com" && !host_lower.contains("gitlab") {
        return None;
    }
    let path = normalize_path(path);
    if !path.contains('/') || path.is_empty() {
        return None;
    }
    Some(GitLabRepo {
        host: host_lower,
        path: path.to_string(),
    })
}

/// Parse a Bitbucket Cloud URL into a [`BitbucketRepo`]. Bitbucket
/// Server (the on-prem product) is not supported — it's been dying
/// for years and uses a different URL scheme.
pub fn parse_bitbucket_remote(url: &str) -> Option<BitbucketRepo> {
    let (host, path) = split_host_and_path(url)?;
    if !host.eq_ignore_ascii_case("bitbucket.org") {
        return None;
    }
    let path = normalize_path(path);
    let (owner, repo) = path.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some(BitbucketRepo {
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

/// Parse a Gitea / Forgejo / Codeberg URL into a [`GiteaRepo`].
/// Detection is heuristic: codeberg.org is recognized explicitly;
/// other hosts must contain `gitea` or `forgejo` in the hostname.
pub fn parse_gitea_remote(url: &str) -> Option<GiteaRepo> {
    let (host, path) = split_host_and_path(url)?;
    let host_lower = host.to_ascii_lowercase();
    let recognized = host_lower == "codeberg.org"
        || host_lower.contains("gitea")
        || host_lower.contains("forgejo");
    if !recognized {
        return None;
    }
    let path = normalize_path(path);
    let (owner, repo) = path.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some(GiteaRepo {
        host: host_lower,
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

/// Dispatch a URL to the appropriate forge parser. Order matters when
/// hosts could overlap (none currently do, but the ordering is
/// stable: most specific first).
pub fn parse_forge_url(url: &str) -> Option<Box<dyn Forge>> {
    if let Some(r) = parse_github_remote(url) {
        return Some(Box::new(r));
    }
    if let Some(r) = parse_gitlab_remote(url) {
        return Some(Box::new(r));
    }
    if let Some(r) = parse_bitbucket_remote(url) {
        return Some(Box::new(r));
    }
    if let Some(r) = parse_gitea_remote(url) {
        return Some(Box::new(r));
    }
    None
}

/// Resolve the GitHub repo for a working tree by inspecting `git remote
/// get-url <preferred>` (defaulting to `origin`). Returns `None` when
/// no GitHub remote is configured. Used by the GitHub PR enricher,
/// which is currently the only fully-implemented enrichment backend;
/// the compare-URL footer uses [`detect_forge`] for broader coverage.
pub async fn detect_github_repo(repo_root: &Path) -> Option<GitHubRepo> {
    for remote in ["origin", "upstream"] {
        if let Some(url) = git_remote_url(repo_root, remote).await
            && let Some(parsed) = parse_github_remote(&url)
        {
            return Some(parsed);
        }
    }
    None
}

/// Resolve the [`Forge`] backing the working tree by inspecting
/// `origin` (and falling back to `upstream`). Returns `None` when no
/// recognized forge URL is configured.
pub async fn detect_forge(repo_root: &Path) -> Option<Box<dyn Forge>> {
    for remote in ["origin", "upstream"] {
        if let Some(url) = git_remote_url(repo_root, remote).await
            && let Some(forge) = parse_forge_url(&url)
        {
            return Some(forge);
        }
    }
    None
}

async fn git_remote_url(repo_root: &Path, remote: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", remote])
        .current_dir(repo_root)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_https_with_dot_git() {
        let p = parse_github_remote("https://github.com/foo/bar.git").unwrap();
        assert_eq!(p.owner, "foo");
        assert_eq!(p.repo, "bar");
    }

    #[test]
    fn parses_https_without_dot_git() {
        let p = parse_github_remote("https://github.com/foo/bar").unwrap();
        assert_eq!(p.repo, "bar");
    }

    #[test]
    fn parses_https_with_trailing_slash() {
        let p = parse_github_remote("https://github.com/foo/bar/").unwrap();
        assert_eq!(p.repo, "bar");
    }

    #[test]
    fn parses_ssh_at_form() {
        let p = parse_github_remote("git@github.com:foo/bar.git").unwrap();
        assert_eq!(p.owner, "foo");
        assert_eq!(p.repo, "bar");
    }

    #[test]
    fn parses_ssh_url_form() {
        let p = parse_github_remote("ssh://git@github.com/foo/bar.git").unwrap();
        assert_eq!(p.owner, "foo");
        assert_eq!(p.repo, "bar");
    }

    #[test]
    fn parses_git_protocol_form() {
        let p = parse_github_remote("git://github.com/foo/bar").unwrap();
        assert_eq!(p.owner, "foo");
    }

    #[test]
    fn rejects_non_github_remote() {
        assert!(parse_github_remote("https://gitlab.com/foo/bar").is_none());
        assert!(parse_github_remote("https://example.com/x/y").is_none());
    }

    #[test]
    fn rejects_malformed() {
        assert!(parse_github_remote("https://github.com/").is_none());
        assert!(parse_github_remote("https://github.com/foo").is_none());
        assert!(parse_github_remote("not a url").is_none());
    }

    #[test]
    fn handles_owner_with_hyphens() {
        let p = parse_github_remote("https://github.com/the-owner/the-repo.git").unwrap();
        assert_eq!(p.owner, "the-owner");
        assert_eq!(p.repo, "the-repo");
    }

    #[test]
    fn compare_url_with_both_refs() {
        let r = GitHubRepo {
            owner: "foo".into(),
            repo: "bar".into(),
        };
        assert_eq!(
            r.compare_url(Some("v1.0.0"), "v1.1.0"),
            "https://github.com/foo/bar/compare/v1.0.0...v1.1.0"
        );
    }

    #[test]
    fn compare_url_falls_back_to_commits_for_initial_release() {
        let r = GitHubRepo {
            owner: "foo".into(),
            repo: "bar".into(),
        };
        assert_eq!(
            r.compare_url(None, "v0.1.0"),
            "https://github.com/foo/bar/commits/v0.1.0"
        );
    }

    // ── GitLab ──────────────────────────────────────────────────────

    #[test]
    fn parses_gitlab_https() {
        let r = parse_gitlab_remote("https://gitlab.com/foo/bar.git").unwrap();
        assert_eq!(r.host, "gitlab.com");
        assert_eq!(r.path, "foo/bar");
    }

    #[test]
    fn parses_gitlab_with_subgroups() {
        let r = parse_gitlab_remote("https://gitlab.com/group/sub/repo.git").unwrap();
        assert_eq!(r.path, "group/sub/repo");
    }

    #[test]
    fn parses_self_hosted_gitlab() {
        let r = parse_gitlab_remote("https://gitlab.example.com/foo/bar.git").unwrap();
        assert_eq!(r.host, "gitlab.example.com");
        assert_eq!(r.path, "foo/bar");
    }

    #[test]
    fn parses_gitlab_ssh() {
        let r = parse_gitlab_remote("git@gitlab.com:foo/bar.git").unwrap();
        assert_eq!(r.host, "gitlab.com");
        assert_eq!(r.path, "foo/bar");
    }

    #[test]
    fn gitlab_compare_url_uses_dash_prefix() {
        let r = GitLabRepo {
            host: "gitlab.com".into(),
            path: "foo/bar".into(),
        };
        assert_eq!(
            r.compare_url(Some("v1.0.0"), "v1.1.0"),
            "https://gitlab.com/foo/bar/-/compare/v1.0.0...v1.1.0"
        );
    }

    #[test]
    fn gitlab_compare_url_with_subgroups() {
        let r = GitLabRepo {
            host: "gitlab.com".into(),
            path: "group/sub/repo".into(),
        };
        assert_eq!(
            r.compare_url(Some("v1"), "v2"),
            "https://gitlab.com/group/sub/repo/-/compare/v1...v2"
        );
    }

    // ── Bitbucket ───────────────────────────────────────────────────

    #[test]
    fn parses_bitbucket() {
        let r = parse_bitbucket_remote("https://bitbucket.org/foo/bar.git").unwrap();
        assert_eq!(r.owner, "foo");
        assert_eq!(r.repo, "bar");
    }

    #[test]
    fn bitbucket_compare_url_reverses_operands_and_uses_two_dots() {
        let r = BitbucketRepo {
            owner: "foo".into(),
            repo: "bar".into(),
        };
        // Note: target before source, two dots — Bitbucket's quirk.
        assert_eq!(
            r.compare_url(Some("v1.0.0"), "v1.1.0"),
            "https://bitbucket.org/foo/bar/branches/compare/v1.1.0..v1.0.0"
        );
    }

    #[test]
    fn bitbucket_initial_release_links_to_tag_commits() {
        let r = BitbucketRepo {
            owner: "foo".into(),
            repo: "bar".into(),
        };
        assert_eq!(
            r.compare_url(None, "v0.1.0"),
            "https://bitbucket.org/foo/bar/commits/tag/v0.1.0"
        );
    }

    // ── Gitea / Codeberg ────────────────────────────────────────────

    #[test]
    fn parses_codeberg() {
        let r = parse_gitea_remote("https://codeberg.org/foo/bar.git").unwrap();
        assert_eq!(r.host, "codeberg.org");
        assert_eq!(r.owner, "foo");
        assert_eq!(r.repo, "bar");
    }

    #[test]
    fn parses_self_hosted_gitea() {
        let r = parse_gitea_remote("https://gitea.example.com/foo/bar.git").unwrap();
        assert_eq!(r.host, "gitea.example.com");
    }

    #[test]
    fn parses_self_hosted_forgejo() {
        let r = parse_gitea_remote("https://forgejo.example.com/foo/bar.git").unwrap();
        assert_eq!(r.host, "forgejo.example.com");
    }

    #[test]
    fn gitea_compare_url() {
        let r = GiteaRepo {
            host: "codeberg.org".into(),
            owner: "foo".into(),
            repo: "bar".into(),
        };
        assert_eq!(
            r.compare_url(Some("v1.0.0"), "v1.1.0"),
            "https://codeberg.org/foo/bar/compare/v1.0.0...v1.1.0"
        );
    }

    // ── Dispatch ────────────────────────────────────────────────────

    #[test]
    fn parse_forge_url_dispatches_to_correct_forge() {
        let cases: &[(&str, &str)] = &[
            ("https://github.com/a/b.git", "github"),
            ("https://gitlab.com/a/b.git", "gitlab"),
            ("https://bitbucket.org/a/b.git", "bitbucket"),
            ("https://codeberg.org/a/b.git", "gitea"),
        ];
        for (url, expected) in cases {
            let f = parse_forge_url(url).unwrap_or_else(|| panic!("no forge for {url}"));
            assert_eq!(f.name(), *expected, "wrong forge for {url}");
        }
    }

    #[test]
    fn parse_forge_url_returns_none_for_unknown_host() {
        assert!(parse_forge_url("https://random.example.com/foo/bar.git").is_none());
    }
}
