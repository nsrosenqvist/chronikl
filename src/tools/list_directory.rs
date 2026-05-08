//! `list_directory` tool — list files/dirs at a path inside the repo.

use std::path::PathBuf;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tools::{ToolError, safe_resolve};

const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone)]
pub struct ListDirectoryTool {
    repo_root: PathBuf,
}

impl ListDirectoryTool {
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListDirectoryArgs {
    /// Relative path inside the repo. Use `.` for the repo root.
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct ListDirectoryOutput {
    pub path: String,
    pub entries: Vec<DirEntry>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct DirEntry {
    pub name: String,
    pub kind: EntryKind,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

impl Tool for ListDirectoryTool {
    const NAME: &'static str = "list_directory";

    type Error = ToolError;
    type Args = ListDirectoryArgs;
    type Output = ListDirectoryOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "List immediate entries of a directory inside the repository. Use \
                          `.` for the repo root. Returns up to 200 entries; large directories \
                          are flagged as truncated."
                .to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(ListDirectoryArgs))
                .unwrap_or(serde_json::Value::Null),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let resolved = if args.path == "." || args.path.is_empty() {
            self.repo_root.clone()
        } else {
            safe_resolve(&self.repo_root, &args.path)?
        };

        let read = std::fs::read_dir(&resolved).map_err(|e| ToolError::Io {
            path: args.path.clone(),
            source: e,
        })?;

        let mut entries: Vec<DirEntry> = Vec::new();
        let mut truncated = false;
        for ent in read {
            let ent = match ent {
                Ok(e) => e,
                Err(_) => continue,
            };
            // Skip the `.git/` directory at the repo root — leaks
            // worktree internals.
            let name = ent.file_name().to_string_lossy().to_string();
            if name == ".git" {
                continue;
            }
            if entries.len() >= MAX_ENTRIES {
                truncated = true;
                break;
            }
            let kind = match ent.file_type() {
                Ok(ft) if ft.is_dir() => EntryKind::Dir,
                Ok(ft) if ft.is_file() => EntryKind::File,
                Ok(ft) if ft.is_symlink() => EntryKind::Symlink,
                _ => EntryKind::Other,
            };
            entries.push(DirEntry { name, kind });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(ListDirectoryOutput {
            path: args.path,
            entries,
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_dir() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "x").unwrap();
        std::fs::write(d.path().join("b.txt"), "x").unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::create_dir_all(d.path().join(".git")).unwrap();
        std::fs::write(d.path().join(".git/config"), "").unwrap();
        d
    }

    #[tokio::test]
    async fn lists_entries_skipping_dot_git() {
        let dir = make_dir();
        let tool = ListDirectoryTool::new(dir.path().to_path_buf());
        let out = tool
            .call(ListDirectoryArgs { path: ".".into() })
            .await
            .unwrap();
        let names: Vec<&str> = out.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "b.txt", "src"]);
        assert!(!out.truncated);
    }

    #[tokio::test]
    async fn rejects_dot_git_traversal() {
        let dir = make_dir();
        let tool = ListDirectoryTool::new(dir.path().to_path_buf());
        let err = tool
            .call(ListDirectoryArgs {
                path: ".git".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InsideGitDir { .. }));
    }

    #[tokio::test]
    async fn missing_dir_is_io_error() {
        let dir = make_dir();
        let tool = ListDirectoryTool::new(dir.path().to_path_buf());
        let err = tool
            .call(ListDirectoryArgs {
                path: "no-such-dir".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Io { .. }));
    }
}
