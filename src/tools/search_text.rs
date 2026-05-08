//! `search_text` tool — regex search across the repo (skipping `.git/`).
//!
//! Returns up to 50 matches with file path, line number, and the
//! matching line. Used by Tier 3 to trace symbols / strings across
//! files when figuring out what a commit really did.

use std::path::PathBuf;

use regex::Regex;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tools::ToolError;

const MAX_MATCHES: usize = 50;
const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct SearchTextTool {
    repo_root: PathBuf,
}

impl SearchTextTool {
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchTextArgs {
    /// Regular expression pattern (Rust regex syntax).
    pub pattern: String,
    /// Optional path glob to restrict the search (e.g. `src/**/*.rs`).
    /// When unset, searches the entire repo (excluding `.git/`).
    #[serde(default)]
    pub path_glob: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchTextOutput {
    pub pattern: String,
    pub matches: Vec<Match>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct Match {
    pub path: String,
    pub line_number: u32,
    pub line: String,
}

impl Tool for SearchTextTool {
    const NAME: &'static str = "search_text";

    type Error = ToolError;
    type Args = SearchTextArgs;
    type Output = SearchTextOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Search the repository for a regex pattern. Returns up to 50 matches \
                          (path, line number, line). Optionally restrict by `path_glob` \
                          (e.g. `src/**/*.rs`). Skips `.git/` and binary-looking files."
                .to_string(),
            parameters: serde_json::to_value(schemars::schema_for!(SearchTextArgs))
                .unwrap_or(serde_json::Value::Null),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let re = Regex::new(&args.pattern).map_err(|e| {
            ToolError::InvalidArgs(format!("invalid regex `{}`: {e}", args.pattern))
        })?;
        let glob = match args.path_glob.as_deref() {
            Some(g) if !g.is_empty() => Some(
                globset::Glob::new(g)
                    .map_err(|e| ToolError::InvalidArgs(format!("invalid path_glob: {e}")))?
                    .compile_matcher(),
            ),
            _ => None,
        };

        let mut matches: Vec<Match> = Vec::new();
        let mut truncated = false;

        let mut stack = vec![self.repo_root.clone()];
        while let Some(dir) = stack.pop() {
            let read = match std::fs::read_dir(&dir) {
                Ok(r) => r,
                Err(_) => continue,
            };
            for ent in read.flatten() {
                let p = ent.path();
                let rel = p.strip_prefix(&self.repo_root).unwrap_or(&p);
                let rel_str = rel.to_string_lossy().to_string();

                if rel.components().any(|c| c.as_os_str() == ".git") {
                    continue;
                }

                let ft = match ent.file_type() {
                    Ok(ft) => ft,
                    Err(_) => continue,
                };
                if ft.is_dir() {
                    stack.push(p);
                    continue;
                }
                if !ft.is_file() {
                    continue;
                }

                if let Some(g) = &glob
                    && !g.is_match(rel)
                {
                    continue;
                }

                if let Ok(meta) = ent.metadata()
                    && meta.len() > MAX_FILE_BYTES
                {
                    continue;
                }

                let content = match std::fs::read_to_string(&p) {
                    Ok(c) => c,
                    Err(_) => continue, // probably binary; skip
                };
                for (i, line) in content.lines().enumerate() {
                    if re.is_match(line) {
                        if matches.len() >= MAX_MATCHES {
                            truncated = true;
                            break;
                        }
                        matches.push(Match {
                            path: rel_str.clone(),
                            line_number: (i + 1) as u32,
                            line: line.chars().take(300).collect(),
                        });
                    }
                }
                if truncated {
                    break;
                }
            }
            if truncated {
                break;
            }
        }
        Ok(SearchTextOutput {
            pattern: args.pattern,
            matches,
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(
            d.path().join("src/lib.rs"),
            "pub fn login() {}\nfn other() {}\n",
        )
        .unwrap();
        std::fs::write(d.path().join("README.md"), "Login flow doc.\n").unwrap();
        std::fs::create_dir_all(d.path().join(".git")).unwrap();
        std::fs::write(d.path().join(".git/config"), "login=secret\n").unwrap();
        d
    }

    #[tokio::test]
    async fn finds_matches_skipping_git() {
        let d = make_repo();
        let tool = SearchTextTool::new(d.path().to_path_buf());
        // Case-insensitive so we match both "login" (lib.rs) and "Login" (README).
        let out = tool
            .call(SearchTextArgs {
                pattern: "(?i)login".into(),
                path_glob: None,
            })
            .await
            .unwrap();
        let paths: Vec<&str> = out.matches.iter().map(|m| m.path.as_str()).collect();
        assert!(paths.iter().any(|p| p.ends_with("lib.rs")));
        assert!(paths.iter().any(|p| p.ends_with("README.md")));
        // .git/config has "login=secret" — but should be skipped.
        assert!(!paths.iter().any(|p| p.contains(".git")));
    }

    #[tokio::test]
    async fn glob_restricts_search() {
        let d = make_repo();
        let tool = SearchTextTool::new(d.path().to_path_buf());
        let out = tool
            .call(SearchTextArgs {
                pattern: "login".into(),
                path_glob: Some("**/*.rs".into()),
            })
            .await
            .unwrap();
        for m in &out.matches {
            assert!(m.path.ends_with(".rs"), "unexpected match: {}", m.path);
        }
    }

    #[tokio::test]
    async fn invalid_regex_is_invalid_args() {
        let d = make_repo();
        let tool = SearchTextTool::new(d.path().to_path_buf());
        let err = tool
            .call(SearchTextArgs {
                pattern: "[unclosed".into(),
                path_glob: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgs(_)));
    }
}
