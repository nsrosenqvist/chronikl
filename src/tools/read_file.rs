//! `read_file` tool — read a file from the repo, optionally a line range.
//!
//! Args:
//! - `path`: relative path inside repo
//! - `start_line` (optional, 1-based)
//! - `end_line` (optional, 1-based, inclusive)
//!
//! Returns: file content (trimmed if range given) or an error.
//! Hard cap on output size to avoid blowing the model's context window
//! when a malicious or curious model asks for a huge file.

use std::path::PathBuf;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tools::{ToolError, safe_resolve};

const MAX_FILE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub struct ReadFileTool {
    repo_root: PathBuf,
}

impl ReadFileTool {
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    /// Relative path inside the repo.
    pub path: String,
    /// Optional 1-based starting line (inclusive).
    #[serde(default)]
    pub start_line: Option<usize>,
    /// Optional 1-based ending line (inclusive).
    #[serde(default)]
    pub end_line: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ReadFileOutput {
    pub path: String,
    pub content: String,
    pub truncated: bool,
}

impl Tool for ReadFileTool {
    const NAME: &'static str = "read_file";

    type Error = ToolError;
    type Args = ReadFileArgs;
    type Output = ReadFileOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Read a file from the repository. Optionally restrict to a 1-based \
                          line range (start_line, end_line). Returns the file content. \
                          Refuses paths outside the repo or under .git/."
                .to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(ReadFileArgs))
                .unwrap_or(serde_json::Value::Null),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let resolved = safe_resolve(&self.repo_root, &args.path)?;
        let content = std::fs::read_to_string(&resolved).map_err(|e| ToolError::Io {
            path: args.path.clone(),
            source: e,
        })?;

        let (slice, truncated_lines) = match (args.start_line, args.end_line) {
            (None, None) => (content.clone(), false),
            (start, end) => slice_lines(&content, start, end),
        };

        let mut truncated = truncated_lines;
        let body = if slice.len() > MAX_FILE_BYTES {
            truncated = true;
            format!(
                "{}\n\n[…truncated; original was {} bytes]",
                &slice[..MAX_FILE_BYTES],
                slice.len()
            )
        } else {
            slice
        };

        Ok(ReadFileOutput {
            path: args.path,
            content: body,
            truncated,
        })
    }
}

fn slice_lines(content: &str, start: Option<usize>, end: Option<usize>) -> (String, bool) {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let start = start.unwrap_or(1).max(1);
    let end = end.unwrap_or(total).min(total).max(start);
    if start > total {
        return (String::new(), true);
    }
    let slice = &lines[start - 1..end];
    let truncated = end < total;
    (slice.join("\n"), truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(dir: &std::path::Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, body).unwrap();
    }

    #[tokio::test]
    async fn reads_a_file() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "src/lib.rs", "pub fn x() {}\n");
        let tool = ReadFileTool::new(dir.path().to_path_buf());
        let out = tool
            .call(ReadFileArgs {
                path: "src/lib.rs".into(),
                start_line: None,
                end_line: None,
            })
            .await
            .unwrap();
        assert_eq!(out.content, "pub fn x() {}\n");
        assert!(!out.truncated);
    }

    #[tokio::test]
    async fn slice_lines_is_inclusive() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "f.txt", "1\n2\n3\n4\n5\n");
        let tool = ReadFileTool::new(dir.path().to_path_buf());
        let out = tool
            .call(ReadFileArgs {
                path: "f.txt".into(),
                start_line: Some(2),
                end_line: Some(4),
            })
            .await
            .unwrap();
        assert_eq!(out.content, "2\n3\n4");
        assert!(out.truncated, "lines past the slice exist → truncated=true");
    }

    #[tokio::test]
    async fn rejects_paths_outside_repo() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ReadFileTool::new(dir.path().to_path_buf());
        let err = tool
            .call(ReadFileArgs {
                path: "../etc/passwd".into(),
                start_line: None,
                end_line: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::EscapesRoot { .. }));
    }

    #[tokio::test]
    async fn rejects_dot_git() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ReadFileTool::new(dir.path().to_path_buf());
        let err = tool
            .call(ReadFileArgs {
                path: ".git/config".into(),
                start_line: None,
                end_line: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InsideGitDir { .. }));
    }

    #[tokio::test]
    async fn missing_file_is_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ReadFileTool::new(dir.path().to_path_buf());
        let err = tool
            .call(ReadFileArgs {
                path: "no/such".into(),
                start_line: None,
                end_line: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Io { .. }));
    }
}
