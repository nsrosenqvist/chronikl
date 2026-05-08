//! Read-only tools the Tier 3 agent loop registers with rig-core.
//!
//! Each tool implements rig's `Tool` trait so the model can call it via
//! its native tool-calling interface. All tools are read-only — the
//! release-notes use case never needs to mutate the working tree.
//!
//! Path safety: every tool that accepts a path validates that the
//! resolved path stays inside the configured `repo_root`. We also
//! refuse paths under `.git/` and reject symlinks that escape the
//! repo. This is hardened because tool args come straight from the
//! model and shouldn't be trusted.

pub mod budget;
pub mod git_show;
pub mod list_directory;
pub mod read_file;
pub mod search_text;
pub mod submit_classification;

pub use git_show::GitShowTool;
pub use list_directory::ListDirectoryTool;
pub use read_file::ReadFileTool;
pub use search_text::SearchTextTool;
pub use submit_classification::{
    SUBMIT_CLASSIFICATION_TOOL_NAME, SubmitClassificationTool, Tier3Verdict,
};

use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("invalid path '{path}': {reason}")]
    InvalidPath { path: String, reason: String },
    #[error("path '{path}' resolves outside the repo root")]
    EscapesRoot { path: String },
    #[error("path '{path}' is under .git/")]
    InsideGitDir { path: String },
    #[error("io error on '{path}': {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid args: {0}")]
    InvalidArgs(String),
    #[error("budget exhausted: {0}")]
    BudgetExhausted(String),
    #[error("other: {0}")]
    Other(String),
}

/// Resolve a user-supplied path under `repo_root`, enforcing:
/// - the path doesn't escape the root via `..`
/// - the path doesn't traverse into `.git/`
/// - if the path or any of its ancestors are symlinks, the resolved
///   target stays inside the (canonicalized) repo root
///
/// The symlink check uses `canonicalize`, which requires the path to
/// exist. When `repo_root` itself doesn't exist (only happens in unit
/// tests with synthetic roots), the check is skipped — we still
/// return the syntactically-validated joined path.
pub fn safe_resolve(repo_root: &Path, rel: &str) -> Result<PathBuf, ToolError> {
    if rel.is_empty() {
        return Err(ToolError::InvalidPath {
            path: rel.to_string(),
            reason: "path is empty".into(),
        });
    }

    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err(ToolError::InvalidPath {
            path: rel.to_string(),
            reason: "absolute paths are not accepted".into(),
        });
    }

    // Reject any `..` component up front; canonicalize alone isn't
    // sufficient because it requires the path to exist, and we want
    // a fast fail before touching the filesystem.
    if rel_path.components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::RootDir
        )
    }) {
        return Err(ToolError::EscapesRoot {
            path: rel.to_string(),
        });
    }

    // Reject `.git` traversal at any depth.
    if rel_path
        .components()
        .any(|c| c.as_os_str() == ".git" || c.as_os_str() == ".git/")
    {
        return Err(ToolError::InsideGitDir {
            path: rel.to_string(),
        });
    }

    let joined = repo_root.join(rel_path);

    // Symlink defense: if the canonical form of the resolved path
    // exists and points outside the canonical repo root, refuse.
    // Skip when the repo root itself doesn't canonicalize (synthetic
    // test paths); the syntactic checks above already cover those.
    if let Ok(canon_root) = repo_root.canonicalize() {
        match joined.canonicalize() {
            Ok(canon) => {
                if !canon.starts_with(&canon_root) {
                    return Err(ToolError::EscapesRoot {
                        path: rel.to_string(),
                    });
                }
                return Ok(canon);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Path doesn't exist — let the caller surface the I/O
                // error from the actual operation. Return the joined
                // (non-canonical) path.
            }
            Err(e) => {
                return Err(ToolError::Io {
                    path: rel.to_string(),
                    source: e,
                });
            }
        }
    }

    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_resolve_accepts_simple_relative() {
        let root = Path::new("/repo");
        let p = safe_resolve(root, "src/lib.rs").unwrap();
        assert_eq!(p, PathBuf::from("/repo/src/lib.rs"));
    }

    #[test]
    fn safe_resolve_rejects_empty() {
        assert!(safe_resolve(Path::new("/repo"), "").is_err());
    }

    #[test]
    fn safe_resolve_rejects_absolute() {
        assert!(matches!(
            safe_resolve(Path::new("/repo"), "/etc/passwd"),
            Err(ToolError::InvalidPath { .. })
        ));
    }

    #[test]
    fn safe_resolve_rejects_parent_dir() {
        assert!(matches!(
            safe_resolve(Path::new("/repo"), "../etc/passwd"),
            Err(ToolError::EscapesRoot { .. })
        ));
        assert!(matches!(
            safe_resolve(Path::new("/repo"), "src/../../etc"),
            Err(ToolError::EscapesRoot { .. })
        ));
    }

    #[test]
    fn safe_resolve_rejects_dot_git() {
        assert!(matches!(
            safe_resolve(Path::new("/repo"), ".git/config"),
            Err(ToolError::InsideGitDir { .. })
        ));
        assert!(matches!(
            safe_resolve(Path::new("/repo"), "src/.git/refs"),
            Err(ToolError::InsideGitDir { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn safe_resolve_rejects_in_repo_symlink_to_outside() {
        use std::fs;
        use std::os::unix::fs::symlink;

        let outer = tempfile::tempdir().unwrap();
        let target = outer.path().join("secret.txt");
        fs::write(&target, "shh").unwrap();

        let repo = tempfile::tempdir().unwrap();
        let link = repo.path().join("escape");
        symlink(&target, &link).unwrap();

        // Symlink resolves outside repo root → must be rejected.
        let err = safe_resolve(repo.path(), "escape").unwrap_err();
        assert!(
            matches!(err, ToolError::EscapesRoot { .. }),
            "expected EscapesRoot, got {err:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn safe_resolve_accepts_in_repo_symlink_inside_repo() {
        use std::fs;
        use std::os::unix::fs::symlink;

        let repo = tempfile::tempdir().unwrap();
        let real = repo.path().join("real.txt");
        fs::write(&real, "ok").unwrap();
        let link = repo.path().join("alias");
        symlink(&real, &link).unwrap();

        // Symlink resolves to a sibling inside the repo → accepted.
        let resolved = safe_resolve(repo.path(), "alias").unwrap();
        assert_eq!(resolved, real.canonicalize().unwrap());
    }
}
