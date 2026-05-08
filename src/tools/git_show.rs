//! `git_show` tool — view another commit's metadata + diff.
//!
//! Lets the Tier 3 agent cross-reference sibling commits in the same
//! release. The commit it's classifying is already in the prompt; this
//! tool answers "what did the surrounding commits do?" without needing
//! to enumerate history up front.
//!
//! Args: `sha` (any hex prefix git accepts; 4–64 chars).
//!
//! Returns the standard `git show` output (commit header + diff),
//! capped at `MAX_OUTPUT_BYTES` to avoid blowing the model's context
//! window on large refactors.

use std::path::PathBuf;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::tools::ToolError;

const MAX_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub struct GitShowTool {
    repo_root: PathBuf,
}

impl GitShowTool {
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GitShowArgs {
    /// Commit SHA (any hex prefix git accepts, 4–64 chars).
    pub sha: String,
}

#[derive(Debug, Serialize)]
pub struct GitShowOutput {
    pub sha: String,
    pub content: String,
    pub truncated: bool,
}

impl Tool for GitShowTool {
    const NAME: &'static str = "git_show";

    type Error = ToolError;
    type Args = GitShowArgs;
    type Output = GitShowOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "View another commit's metadata and diff by SHA. Useful for \
                          cross-referencing sibling commits in the same release \
                          (e.g. \"did the previous commit also touch this file?\"). \
                          Accepts any hex SHA prefix git resolves; refuses non-hex \
                          input. Output is capped at 256 KB."
                .to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(GitShowArgs))
                .unwrap_or(serde_json::Value::Null),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        validate_sha(&args.sha)?;

        let output = Command::new("git")
            .args(["show", "--no-color", "--no-renames", &args.sha])
            .current_dir(&self.repo_root)
            .output()
            .await
            .map_err(|e| ToolError::Io {
                path: args.sha.clone(),
                source: e,
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(ToolError::Other(format!(
                "git show {} failed: {stderr}",
                args.sha
            )));
        }

        let raw = String::from_utf8_lossy(&output.stdout);
        let (content, truncated) = if raw.len() > MAX_OUTPUT_BYTES {
            (
                format!(
                    "{}\n\n[…truncated; original was {} bytes]",
                    &raw[..MAX_OUTPUT_BYTES],
                    raw.len()
                ),
                true,
            )
        } else {
            (raw.to_string(), false)
        };

        Ok(GitShowOutput {
            sha: args.sha,
            content,
            truncated,
        })
    }
}

/// Reject anything that isn't a hex SHA prefix. Git would accept full
/// refs (`HEAD`, `main`, `v1.0.0`), but limiting to hex closes the
/// door on accidental ref-name surprises and keeps the contract
/// narrow ("look at this commit by SHA").
fn validate_sha(sha: &str) -> Result<(), ToolError> {
    if !(4..=64).contains(&sha.len()) {
        return Err(ToolError::InvalidArgs(format!(
            "sha must be 4–64 hex chars, got {} chars",
            sha.len()
        )));
    }
    if !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ToolError::InvalidArgs(format!(
            "sha '{sha}' is not a hex string"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_git(repo: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
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

    fn make_repo() -> (tempfile::TempDir, std::path::PathBuf, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        run_git(&path, &["init", "--initial-branch=main"]);
        run_git(&path, &["config", "user.name", "ada"]);
        run_git(&path, &["config", "user.email", "ada@x"]);
        run_git(&path, &["config", "commit.gpgsign", "false"]);
        std::fs::write(path.join("a.txt"), "hello\n").unwrap();
        run_git(&path, &["add", "a.txt"]);
        run_git(&path, &["commit", "-m", "feat: add greeting"]);
        let sha = String::from_utf8_lossy(
            &std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&path)
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();
        (dir, path, sha)
    }

    #[tokio::test]
    async fn shows_a_commit_by_full_sha() {
        let (_dir, repo, sha) = make_repo();
        let tool = GitShowTool::new(repo);
        let out = tool.call(GitShowArgs { sha: sha.clone() }).await.unwrap();
        assert!(out.content.contains("feat: add greeting"));
        assert!(out.content.contains("+hello"));
        assert!(!out.truncated);
        assert_eq!(out.sha, sha);
    }

    #[tokio::test]
    async fn shows_a_commit_by_short_sha() {
        let (_dir, repo, sha) = make_repo();
        let tool = GitShowTool::new(repo);
        let short = &sha[..7];
        let out = tool
            .call(GitShowArgs {
                sha: short.to_string(),
            })
            .await
            .unwrap();
        assert!(out.content.contains("feat: add greeting"));
    }

    #[tokio::test]
    async fn rejects_non_hex_sha() {
        let (_dir, repo, _) = make_repo();
        let tool = GitShowTool::new(repo);
        let err = tool
            .call(GitShowArgs { sha: "HEAD".into() })
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn rejects_too_short_sha() {
        let (_dir, repo, _) = make_repo();
        let tool = GitShowTool::new(repo);
        let err = tool
            .call(GitShowArgs { sha: "abc".into() })
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn unknown_sha_surfaces_git_error() {
        let (_dir, repo, _) = make_repo();
        let tool = GitShowTool::new(repo);
        let err = tool
            .call(GitShowArgs {
                sha: "deadbeef".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Other(_)));
    }
}
