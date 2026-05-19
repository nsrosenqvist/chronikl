//! Project metadata detection for the prose pass.
//!
//! Looks at the repository root to find:
//! - A `description` field in `Cargo.toml` / `package.json` /
//!   `pyproject.toml`.
//! - A README file (`README.md`, `README.rst`, `README.txt`, `README`)
//!   and extracts its intro paragraphs.
//!
//! The result is folded into the prose-pass user prompt so the model has
//! a real "what this project is" signal — crucial for initial releases
//! where the commit history alone doesn't describe the product.

use std::fs;
use std::path::{Path, PathBuf};

/// Resolved project context passed to the prose pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectContext {
    /// One-line project description from a language manifest or a
    /// TOML/CLI override.
    pub description: Option<String>,
    /// README intro (everything before the first `##` heading, with
    /// badges and YAML front matter stripped), truncated.
    pub readme_intro: Option<String>,
}

impl ProjectContext {
    pub fn is_empty(&self) -> bool {
        self.description.is_none() && self.readme_intro.is_none()
    }
}

/// Inputs for [`resolve`]. CLI overrides win over TOML; TOML wins over
/// auto-detection.
#[derive(Debug, Clone, Copy, Default)]
pub struct ResolveInputs<'a> {
    /// `--project-description "..."`. Highest precedence for description.
    pub cli_description: Option<&'a str>,
    /// `--readme <PATH>`. Highest precedence for README path.
    pub cli_readme: Option<&'a Path>,
    /// `--no-readme`. Disables README detection entirely.
    pub cli_no_readme: bool,
    /// `[project].description` from TOML.
    pub toml_description: Option<&'a str>,
    /// `[project].readme = "..."` from TOML.
    pub toml_readme: Option<&'a Path>,
    /// `[project].no_readme = true` from TOML.
    pub toml_no_readme: bool,
}

/// Resolve a [`ProjectContext`] for the given repo, applying precedence
/// CLI > TOML > auto-detect.
pub fn resolve(repo_root: &Path, inputs: ResolveInputs<'_>) -> ProjectContext {
    let description = inputs
        .cli_description
        .map(|s| s.to_string())
        .or_else(|| inputs.toml_description.map(|s| s.to_string()))
        .or_else(|| manifest_description(repo_root))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let no_readme = inputs.cli_no_readme || inputs.toml_no_readme;
    let readme_intro = if no_readme {
        None
    } else {
        let path: Option<PathBuf> = inputs
            .cli_readme
            .map(|p| p.to_path_buf())
            .or_else(|| inputs.toml_readme.map(|p| p.to_path_buf()))
            .or_else(|| find_readme(repo_root));
        path.and_then(|p| read_readme_intro(&p))
    };

    ProjectContext {
        description,
        readme_intro,
    }
}

/// Hard cap on README intro length sent to the model.
const README_INTRO_MAX_CHARS: usize = 800;

/// Hard cap on manifest description length. Manifests are usually
/// concise but some authors over-share.
const DESCRIPTION_MAX_CHARS: usize = 300;

/// Filename candidates we search for, in priority order.
const README_CANDIDATES: &[&str] = &["README.md", "README.rst", "README.txt", "README"];

/// Find a README file in `repo_root`, searching common variants in
/// priority order. Case-sensitive on disk.
pub fn find_readme(repo_root: &Path) -> Option<PathBuf> {
    README_CANDIDATES
        .iter()
        .map(|name| repo_root.join(name))
        .find(|path| path.is_file())
}

/// Read the README at `path` and extract its intro.
///
/// Strips a leading YAML front-matter block and badge-style image lines,
/// then takes everything before the first `##` heading and caps at
/// [`README_INTRO_MAX_CHARS`]. Returns `None` when the file can't be
/// read or the cleaned intro is empty.
pub fn read_readme_intro(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let cleaned = clean_readme_intro(&text);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Detect a project description from a language manifest in `repo_root`.
/// Tries `Cargo.toml`, then `package.json`, then `pyproject.toml`.
pub fn manifest_description(repo_root: &Path) -> Option<String> {
    cargo_toml_description(repo_root)
        .or_else(|| package_json_description(repo_root))
        .or_else(|| pyproject_description(repo_root))
        .map(|d| truncate(&d, DESCRIPTION_MAX_CHARS))
}

fn cargo_toml_description(repo_root: &Path) -> Option<String> {
    let text = fs::read_to_string(repo_root.join("Cargo.toml")).ok()?;
    let parsed: toml::Value = toml::from_str(&text).ok()?;
    let desc = parsed
        .get("package")?
        .as_table()?
        .get("description")?
        .as_str()?
        .trim();
    if desc.is_empty() {
        None
    } else {
        Some(desc.to_string())
    }
}

fn package_json_description(repo_root: &Path) -> Option<String> {
    let text = fs::read_to_string(repo_root.join("package.json")).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&text).ok()?;
    let desc = parsed.get("description")?.as_str()?.trim();
    if desc.is_empty() {
        None
    } else {
        Some(desc.to_string())
    }
}

fn pyproject_description(repo_root: &Path) -> Option<String> {
    let text = fs::read_to_string(repo_root.join("pyproject.toml")).ok()?;
    let parsed: toml::Value = toml::from_str(&text).ok()?;
    // PEP 621: [project] description
    let desc = parsed
        .get("project")?
        .as_table()?
        .get("description")?
        .as_str()?
        .trim();
    if desc.is_empty() {
        None
    } else {
        Some(desc.to_string())
    }
}

fn clean_readme_intro(raw: &str) -> String {
    let after_front_matter = strip_yaml_front_matter(raw);
    let mut intro = String::new();
    for line in after_front_matter.lines() {
        let trimmed = line.trim_start();
        // Stop at any heading deeper than level 1. Level-1 headings
        // (`# project-name`) are the project title and worth keeping.
        if trimmed.starts_with("##") {
            break;
        }
        if is_badge_line(trimmed) {
            continue;
        }
        intro.push_str(line);
        intro.push('\n');
    }
    let normalized = collapse_blank_runs(intro.trim());
    truncate(&normalized, README_INTRO_MAX_CHARS)
}

fn strip_yaml_front_matter(s: &str) -> &str {
    if let Some(rest) = s.strip_prefix("---\n") {
        if let Some(end_idx) = rest.find("\n---\n") {
            return &rest[end_idx + 5..];
        }
    }
    s
}

/// A line is "badge-style" if its content is entirely a Markdown image
/// or image-link expression. Cheap heuristic: starts with `![` or `[![`
/// and ends with `)`.
fn is_badge_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    (trimmed.starts_with("[![") || trimmed.starts_with("![")) && trimmed.ends_with(')')
}

fn collapse_blank_runs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_blank = false;
    for line in s.lines() {
        if line.trim().is_empty() {
            if !last_was_blank && !out.is_empty() {
                out.push('\n');
            }
            last_was_blank = true;
        } else {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            if last_was_blank {
                out.push('\n');
            }
            out.push_str(line);
            last_was_blank = false;
        }
    }
    out
}

fn truncate(s: &str, max_chars: usize) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let prefix: String = trimmed.chars().take(max_chars).collect();
    format!("{prefix}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn find_readme_prefers_md() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "README", "plain");
        write(dir.path(), "README.md", "markdown");
        let found = find_readme(dir.path()).unwrap();
        assert_eq!(found.file_name().unwrap(), "README.md");
    }

    #[test]
    fn find_readme_returns_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_readme(dir.path()).is_none());
    }

    #[test]
    fn read_readme_intro_keeps_title_and_intro_paragraph() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "README.md",
            "# chronikl\n\
             \n\
             **AI-powered release notes for your team.** Turn commits into prose.\n\
             \n\
             ## Install\n\
             \n\
             cargo install chronikl\n",
        );
        let intro = read_readme_intro(&path).unwrap();
        assert!(intro.contains("# chronikl"));
        assert!(intro.contains("Turn commits into prose"));
        // `## Install` and everything after must be cut.
        assert!(!intro.contains("## Install"));
        assert!(!intro.contains("cargo install"));
    }

    #[test]
    fn read_readme_intro_strips_badge_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "README.md",
            "# chronikl\n\
             \n\
             [![Crates.io](https://img.shields.io/crates/v/chronikl)](https://crates.io/crates/chronikl)\n\
             [![CI](https://example.com/ci.svg)](https://example.com/ci)\n\
             ![Standalone badge](https://example.com/x.svg)\n\
             \n\
             The actual tagline.\n\
             \n\
             ## Install\n",
        );
        let intro = read_readme_intro(&path).unwrap();
        assert!(intro.contains("# chronikl"));
        assert!(intro.contains("The actual tagline."));
        assert!(!intro.contains("Crates.io"));
        assert!(!intro.contains("CI"));
        assert!(!intro.contains("Standalone badge"));
    }

    #[test]
    fn read_readme_intro_strips_yaml_front_matter() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "README.md",
            "---\n\
             title: chronikl\n\
             layout: project\n\
             ---\n\
             # chronikl\n\
             \n\
             Real content.\n",
        );
        let intro = read_readme_intro(&path).unwrap();
        assert!(intro.contains("# chronikl"));
        assert!(intro.contains("Real content"));
        assert!(!intro.contains("layout:"));
        assert!(!intro.contains("title: chronikl"));
    }

    #[test]
    fn read_readme_intro_truncates_to_cap() {
        let dir = tempfile::tempdir().unwrap();
        let body = "x".repeat(README_INTRO_MAX_CHARS * 2);
        let path = write(dir.path(), "README.md", &body);
        let intro = read_readme_intro(&path).unwrap();
        assert!(intro.chars().count() <= README_INTRO_MAX_CHARS + 1); // +1 for ellipsis
        assert!(intro.ends_with('…'));
    }

    #[test]
    fn read_readme_intro_returns_none_for_empty_after_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "README.md",
            "## Install\n\
             \n\
             cargo install\n",
        );
        // First line is `##`, so the intro is empty.
        assert!(read_readme_intro(&path).is_none());
    }

    #[test]
    fn cargo_toml_description_reads_package_section() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Cargo.toml",
            "[package]\n\
             name = \"x\"\n\
             version = \"0.1.0\"\n\
             description = \"A small CLI\"\n",
        );
        assert_eq!(
            manifest_description(dir.path()).as_deref(),
            Some("A small CLI")
        );
    }

    #[test]
    fn package_json_description_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"x","description":"A node CLI"}"#,
        );
        assert_eq!(
            manifest_description(dir.path()).as_deref(),
            Some("A node CLI")
        );
    }

    #[test]
    fn pyproject_description_reads_pep621() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "pyproject.toml",
            "[project]\n\
             name = \"x\"\n\
             description = \"A python CLI\"\n",
        );
        assert_eq!(
            manifest_description(dir.path()).as_deref(),
            Some("A python CLI")
        );
    }

    #[test]
    fn manifest_description_prefers_cargo_over_others() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Cargo.toml",
            "[package]\nname=\"x\"\nversion=\"0\"\ndescription=\"rust\"\n",
        );
        write(
            dir.path(),
            "package.json",
            r#"{"name":"x","description":"node"}"#,
        );
        assert_eq!(manifest_description(dir.path()).as_deref(), Some("rust"));
    }

    #[test]
    fn manifest_description_returns_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert!(manifest_description(dir.path()).is_none());
    }

    #[test]
    fn manifest_description_truncates_excessive() {
        let dir = tempfile::tempdir().unwrap();
        let long = "x".repeat(DESCRIPTION_MAX_CHARS * 2);
        write(
            dir.path(),
            "Cargo.toml",
            &format!("[package]\nname=\"x\"\nversion=\"0\"\ndescription=\"{long}\"\n"),
        );
        let desc = manifest_description(dir.path()).unwrap();
        assert!(desc.chars().count() <= DESCRIPTION_MAX_CHARS + 1);
        assert!(desc.ends_with('…'));
    }

    #[test]
    fn project_context_is_empty_helper() {
        assert!(ProjectContext::default().is_empty());
        let with_desc = ProjectContext {
            description: Some("x".into()),
            readme_intro: None,
        };
        assert!(!with_desc.is_empty());
    }
}
