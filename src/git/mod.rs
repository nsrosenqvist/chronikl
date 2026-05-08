//! Git driver: range resolution and commit log parsing.
//!
//! # Bounded Context: Git Layer
//!
//! Spawns `git` as a subprocess and translates its output into the
//! domain [`Commit`] model. Two-pass approach for robustness — one
//! `git log` for metadata, one for the per-commit file list — joined
//! by SHA in Rust.

pub mod conventional;
pub mod merge_style;

use std::path::Path;

use thiserror::Error;
use tokio::process::Command;

use crate::models::Commit;

/// Field separator within a commit record (ASCII Unit Separator).
const FS: char = '\x1f';

/// Record terminator between commits (ASCII Record Separator).
const RS: char = '\x1e';

#[derive(Debug, Error)]
pub enum GitError {
    #[error("git command failed: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("git exited with status {status}: {stderr}")]
    NonZeroExit { status: i32, stderr: String },
    #[error("malformed git log output: {0}")]
    ParseError(String),
}

/// What range of commits to include.
#[derive(Debug, Clone)]
pub enum RangeSpec {
    /// Auto: previous semver tag → HEAD. If HEAD itself is tagged,
    /// resolves to the tag *before* HEAD.
    Auto,
    /// Latest semver tag → HEAD, even when HEAD is itself tagged.
    SinceLastTag,
    /// User-supplied refs.
    Explicit { from: Option<String>, to: String },
}

/// Concrete refs after resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    /// `None` means "from the start of history".
    pub from: Option<String>,
    pub to: String,
}

impl Range {
    /// Format as a `git log` range argument: `from..to` or just `to`.
    pub fn as_arg(&self) -> String {
        match &self.from {
            Some(from) => format!("{from}..{}", self.to),
            None => self.to.clone(),
        }
    }
}

/// Resolve a [`RangeSpec`] into concrete refs by querying git.
pub async fn resolve_range(repo: &Path, spec: &RangeSpec) -> Result<Range, GitError> {
    match spec {
        RangeSpec::Explicit { from, to } => Ok(Range {
            from: from.clone(),
            to: to.clone(),
        }),
        RangeSpec::Auto => {
            let to = "HEAD".to_string();
            let head_tag = head_tag(repo).await?;
            let from = match head_tag {
                Some(tag) => previous_semver_tag(repo, &tag).await?,
                None => latest_semver_tag(repo).await?,
            };
            Ok(Range { from, to })
        }
        RangeSpec::SinceLastTag => {
            let to = "HEAD".to_string();
            let from = latest_semver_tag(repo).await?;
            Ok(Range { from, to })
        }
    }
}

/// Return the semver-shaped tag pointing at HEAD, if any.
async fn head_tag(repo: &Path) -> Result<Option<String>, GitError> {
    let out = run_git(repo, &["tag", "--points-at", "HEAD"]).await?;
    Ok(out
        .lines()
        .map(str::trim)
        .find(|t| is_semver_tag(t))
        .map(|s| s.to_string()))
}

/// All semver tags in the repo, descending by version sort.
async fn semver_tags_desc(repo: &Path) -> Result<Vec<String>, GitError> {
    let out = run_git(repo, &["tag", "--list", "v*.*.*", "--sort=-v:refname"]).await?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect())
}

/// Latest semver tag in the repo (newest by version sort).
async fn latest_semver_tag(repo: &Path) -> Result<Option<String>, GitError> {
    Ok(semver_tags_desc(repo).await?.into_iter().next())
}

/// Tag immediately preceding `tag` by version sort.
async fn previous_semver_tag(repo: &Path, tag: &str) -> Result<Option<String>, GitError> {
    let mut iter = semver_tags_desc(repo).await?.into_iter();
    while let Some(t) = iter.next() {
        if t == tag {
            return Ok(iter.next());
        }
    }
    Ok(None)
}

fn is_semver_tag(t: &str) -> bool {
    // Match `v\d+\.\d+\.\d+` plus optional pre-release / build metadata.
    let bytes = t.as_bytes();
    if bytes.first() != Some(&b'v') {
        return false;
    }
    let rest = &t[1..];
    let mut dot_count = 0;
    let mut has_digit = false;
    for ch in rest.chars() {
        if ch == '.' {
            if !has_digit {
                return false;
            }
            dot_count += 1;
            has_digit = false;
            if dot_count > 2 {
                // After two dots we accept any pre-release / build tail.
                return has_digit_anywhere(rest);
            }
        } else if ch.is_ascii_digit() {
            has_digit = true;
        } else if dot_count == 2 {
            // We've seen v\d+\.\d+\.\d — anything trailing (e.g. `-rc.1`,
            // `+build`) is fine as long as we already had a digit.
            return has_digit;
        } else {
            return false;
        }
    }
    dot_count == 2 && has_digit
}

fn has_digit_anywhere(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_digit())
}

/// Detect the version bump between `from_ref` and `to_ref`. Recognises
/// both semver and CalVer; refuses to compare across schemes.
pub async fn detect_version_bump(
    repo: &Path,
    from_ref: Option<&str>,
    to_ref: &str,
) -> Result<
    (
        crate::models::VersionBump,
        Option<crate::models::VersionScheme>,
    ),
    GitError,
> {
    use crate::models::VersionBump;
    let to_parsed = resolve_parsed_version(repo, to_ref).await?;
    let from_parsed = match from_ref {
        Some(f) => resolve_parsed_version(repo, f).await?,
        None => None,
    };
    let bump = VersionBump::from_parsed(from_parsed.as_ref(), to_parsed.as_ref());
    let scheme = to_parsed.as_ref().map(|p| p.scheme);
    Ok((bump, scheme))
}

/// Resolve a ref to a [`ParsedVersion`]: parse the ref string directly
/// first, then `git tag --points-at <ref>`, then `None`.
async fn resolve_parsed_version(
    repo: &Path,
    ref_str: &str,
) -> Result<Option<crate::models::ParsedVersion>, GitError> {
    use crate::models::ParsedVersion;
    if let Some(v) = ParsedVersion::parse(ref_str) {
        return Ok(Some(v));
    }
    let out = run_git(repo, &["tag", "--points-at", ref_str]).await?;
    for line in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(v) = ParsedVersion::parse(line) {
            return Ok(Some(v));
        }
    }
    Ok(None)
}

/// Detect what kind of release the given ref represents — stable
/// semver, prerelease (e.g. `v1.0.0-rc.1`), or untagged.
///
/// Tries, in order:
/// 1. Parse `to_ref` directly as a semver tag (cheap; works when the
///    user passed `--to v1.0.0-rc.1`).
/// 2. Query `git tag --points-at <to_ref>` and parse the result.
/// 3. Fall through to [`ReleaseKind::Untagged`].
pub async fn detect_release_kind(
    repo: &Path,
    to_ref: &str,
) -> Result<crate::models::ReleaseKind, GitError> {
    use crate::models::ReleaseKind;

    if let Some(kind) = ReleaseKind::from_tag(to_ref) {
        return Ok(kind);
    }

    let out = run_git(repo, &["tag", "--points-at", to_ref]).await?;
    for line in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(kind) = ReleaseKind::from_tag(line) {
            return Ok(kind);
        }
    }
    Ok(ReleaseKind::Untagged)
}

/// Read the unified diff for a single commit (vs. its first parent).
///
/// Returns an empty string for the root commit (no parent → no diff).
/// Uses `git show --format=` so only the patch text is emitted, with no
/// commit-metadata header to strip.
pub async fn commit_diff(repo: &Path, sha: &str) -> Result<String, GitError> {
    let raw = run_git(
        repo,
        &["show", "--no-color", "--no-renames", "--format=", sha],
    )
    .await?;
    Ok(raw.trim_start_matches('\n').to_string())
}

/// Read commits in the given range.
pub async fn log(repo: &Path, range: &Range) -> Result<Vec<Commit>, GitError> {
    // Pass 1: structured metadata.
    let format = format!(
        "%H{FS}%h{FS}%an{FS}%ae{FS}%aI{FS}%P{FS}%s{FS}%b{RS}",
        FS = FS,
        RS = RS
    );
    let metadata = run_git(
        repo,
        &["log", &range.as_arg(), &format!("--format={format}")],
    )
    .await?;

    // Pass 2: per-commit file list, keyed by SHA.
    let raw_files = run_git(
        repo,
        &[
            "log",
            &range.as_arg(),
            "--name-only",
            "--format=__CHRONIKL_SHA__%H",
        ],
    )
    .await?;
    let files_by_sha = parse_files(&raw_files);

    let mut commits = parse_metadata(&metadata)?;
    for c in &mut commits {
        if let Some(files) = files_by_sha.get(&c.sha) {
            c.files = files.clone();
        }
    }
    Ok(commits)
}

fn parse_metadata(s: &str) -> Result<Vec<Commit>, GitError> {
    let mut commits = Vec::new();
    for record in s.split(RS) {
        let record = record.trim_start_matches('\n');
        if record.trim().is_empty() {
            continue;
        }
        let mut fields = record.split(FS);
        let sha = fields.next().ok_or_else(|| pe("missing sha"))?.to_string();
        let short_sha = fields
            .next()
            .ok_or_else(|| pe("missing short sha"))?
            .to_string();
        let author_name = fields
            .next()
            .ok_or_else(|| pe("missing author name"))?
            .to_string();
        let author_email = fields
            .next()
            .ok_or_else(|| pe("missing author email"))?
            .to_string();
        let author_date = fields
            .next()
            .ok_or_else(|| pe("missing author date"))?
            .to_string();
        let parents_raw = fields.next().ok_or_else(|| pe("missing parents"))?;
        let subject = fields
            .next()
            .ok_or_else(|| pe("missing subject"))?
            .to_string();
        let body = fields
            .next()
            .unwrap_or("")
            .trim_end_matches('\n')
            .to_string();

        let parents = parents_raw
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();

        let conventional = conventional::parse(&subject, &body);
        let breaking = conventional
            .as_ref()
            .map(|c| c.breaking)
            .unwrap_or_else(|| conventional::has_breaking_footer(&body));
        let pr_id = merge_style::extract_pr_id(&subject);

        commits.push(Commit {
            sha,
            short_sha,
            author_name,
            author_email,
            author_date,
            parents,
            subject,
            body,
            files: Vec::new(),
            pr_id,
            conventional,
            breaking,
        });
    }
    Ok(commits)
}

fn parse_files(s: &str) -> std::collections::HashMap<String, Vec<String>> {
    use std::collections::HashMap;
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let mut current: Option<String> = None;
    for line in s.lines() {
        if let Some(sha) = line.strip_prefix("__CHRONIKL_SHA__") {
            current = Some(sha.to_string());
            out.entry(sha.to_string()).or_default();
        } else if !line.trim().is_empty() {
            if let Some(sha) = &current {
                out.entry(sha.clone()).or_default().push(line.to_string());
            }
        }
    }
    out
}

fn pe(msg: &str) -> GitError {
    GitError::ParseError(msg.to_string())
}

async fn run_git(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .await?;
    if !output.status.success() {
        return Err(GitError::NonZeroExit {
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_arg_with_from() {
        let r = Range {
            from: Some("v0.1.0".into()),
            to: "HEAD".into(),
        };
        assert_eq!(r.as_arg(), "v0.1.0..HEAD");
    }

    #[test]
    fn range_arg_without_from() {
        let r = Range {
            from: None,
            to: "HEAD".into(),
        };
        assert_eq!(r.as_arg(), "HEAD");
    }

    #[test]
    fn semver_tag_recognition() {
        assert!(is_semver_tag("v0.1.0"));
        assert!(is_semver_tag("v1.2.3"));
        assert!(is_semver_tag("v1.2.3-rc.1"));
        assert!(is_semver_tag("v1.2.3+build.4"));
        assert!(!is_semver_tag("v1"));
        assert!(!is_semver_tag("v1.2"));
        assert!(!is_semver_tag("1.2.3"));
        assert!(!is_semver_tag("foo"));
    }

    #[test]
    fn parse_metadata_single_commit() {
        let input = format!(
            "abc123def{FS}abc123d{FS}Ada{FS}ada@x.com{FS}2026-01-01T00:00:00+00:00{FS}p1 p2{FS}feat: hello{FS}body line 1\nbody line 2{RS}",
            FS = FS,
            RS = RS
        );
        let commits = parse_metadata(&input).unwrap();
        assert_eq!(commits.len(), 1);
        let c = &commits[0];
        assert_eq!(c.sha, "abc123def");
        assert_eq!(c.short_sha, "abc123d");
        assert_eq!(c.author_name, "Ada");
        assert_eq!(c.author_email, "ada@x.com");
        assert_eq!(c.parents, vec!["p1", "p2"]);
        assert_eq!(c.subject, "feat: hello");
        assert!(c.body.starts_with("body line 1"));
        assert!(c.is_merge());
        let conv = c.conventional.as_ref().unwrap();
        assert_eq!(conv.kind, "feat");
        assert_eq!(conv.description, "hello");
    }

    #[test]
    fn parse_metadata_multiple_commits() {
        let input = format!(
            "sha1{FS}s1{FS}A{FS}a@x{FS}2026-01-01T00:00:00+00:00{FS}p{FS}feat: a{FS}{RS}\nsha2{FS}s2{FS}B{FS}b@x{FS}2026-01-02T00:00:00+00:00{FS}p{FS}fix: b{FS}{RS}",
            FS = FS,
            RS = RS
        );
        let commits = parse_metadata(&input).unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].subject, "feat: a");
        assert_eq!(commits[1].subject, "fix: b");
    }

    #[test]
    fn parse_files_groups_by_sha() {
        let input =
            "__CHRONIKL_SHA__abc\nsrc/lib.rs\nREADME.md\n\n__CHRONIKL_SHA__def\nCargo.toml\n";
        let map = parse_files(input);
        assert_eq!(
            map.get("abc").unwrap(),
            &vec!["src/lib.rs".to_string(), "README.md".to_string()]
        );
        assert_eq!(map.get("def").unwrap(), &vec!["Cargo.toml".to_string()]);
    }

    #[test]
    fn parse_metadata_picks_up_breaking_footer() {
        let input = format!(
            "sha{FS}s{FS}A{FS}a@x{FS}2026-01-01T00:00:00+00:00{FS}p{FS}refactor: rename Foo{FS}This renames Foo to Bar.\n\nBREAKING CHANGE: Foo no longer exists.{RS}",
            FS = FS,
            RS = RS
        );
        let commits = parse_metadata(&input).unwrap();
        assert!(commits[0].breaking);
    }

    #[test]
    fn parse_metadata_picks_up_pr_id() {
        let input = format!(
            "sha{FS}s{FS}A{FS}a@x{FS}2026-01-01T00:00:00+00:00{FS}p{FS}Add login flow (#42){FS}{RS}",
            FS = FS,
            RS = RS
        );
        let commits = parse_metadata(&input).unwrap();
        assert_eq!(commits[0].pr_id, Some(42));
    }
}
