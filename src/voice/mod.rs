//! Voice loading: from a bundled profile, a user-supplied markdown file, or
//! the default profile.
//!
//! # Bounded Context: Voice
//!
//! A "voice" is the system prompt that drives the prose pass. The user
//! supplies one of:
//! - a bundled profile name (`terse`, `prose`, or the alias `default`) via
//!   `--voice <name>` / TOML `voice.profile` → loaded with [`load_bundled`]
//! - a path via `--voice <FILE>` / TOML `voice.path` → loaded with
//!   [`load_from_file`]
//! - nothing → the bundled `terse` voice (the implicit default)
//!
//! Voice files are plain Markdown — no frontmatter, no metadata. The
//! whole file body is used as the system prompt.
//!
//! Inline `--prompt` and TOML `voice.extra_instructions` are *not*
//! applied here; they are appended at the call site (the prose pass) so
//! the underlying voice stays cacheable on its own hash.

use std::path::Path;

use thiserror::Error;

/// Bundled voice contents. Embedded at compile time so a stripped
/// chronikl binary can run without any voice file on disk.
const TERSE_VOICE_MD: &str = include_str!("terse.md");
const PROSE_VOICE_MD: &str = include_str!("prose.md");

/// Public list of bundled profile names, in display order. Used in error
/// messages and `--help` text. Aliases (e.g. `default`) are *not*
/// included.
pub const BUNDLED_NAMES: &[&str] = &["terse", "prose"];

/// A voice — the system prompt used by the prose pass, plus enough
/// metadata to identify it in audit logs and the run header.
///
/// `is_custom` and `bundled_name` are mutually exclusive in practice:
/// `bundled_name` is `Some(_)` iff the voice came from a bundled profile;
/// `is_custom` is `true` iff it was loaded from a user-supplied file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voice {
    pub system_prompt: String,
    pub is_custom: bool,
    /// Canonical bundled profile name (e.g. `"terse"`, `"prose"`) when
    /// loaded from a bundled profile. `None` for custom files.
    pub bundled_name: Option<&'static str>,
}

#[derive(Debug, Error)]
pub enum VoiceError {
    #[error("could not read voice file {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("voice file {path} is empty")]
    Empty { path: String },
    #[error("unknown voice profile `{name}` (known: {})", known.join(", "))]
    UnknownProfile {
        name: String,
        known: &'static [&'static str],
    },
}

/// The bundled `terse` voice — chronikl's implicit default when nothing
/// else is configured. Cannot fail at runtime — `include_str!` would
/// have failed the build if the file were missing.
pub fn default() -> Voice {
    load_bundled("terse").expect("bundled terse voice must exist")
}

/// Map a profile name (or alias) to its canonical name and content.
/// `"default"` is an alias for `"terse"`.
fn bundled_by_name(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "default" | "terse" => Some(("terse", TERSE_VOICE_MD)),
        "prose" => Some(("prose", PROSE_VOICE_MD)),
        _ => None,
    }
}

/// Load a bundled voice profile by name. Accepts the canonical names
/// (`terse`, `prose`) and the `default` alias. Errors with
/// [`VoiceError::UnknownProfile`] for anything else.
pub fn load_bundled(name: &str) -> Result<Voice, VoiceError> {
    match bundled_by_name(name) {
        Some((canonical, body)) => Ok(Voice {
            system_prompt: body.trim().to_string(),
            is_custom: false,
            bundled_name: Some(canonical),
        }),
        None => Err(VoiceError::UnknownProfile {
            name: name.to_string(),
            known: BUNDLED_NAMES,
        }),
    }
}

/// Load a voice from a markdown file. Errors on I/O failure or an empty
/// (whitespace-only) file.
pub fn load_from_file(path: &Path) -> Result<Voice, VoiceError> {
    let raw = std::fs::read_to_string(path).map_err(|e| VoiceError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(VoiceError::Empty {
            path: path.display().to_string(),
        });
    }
    Ok(Voice {
        system_prompt: trimmed.to_string(),
        is_custom: true,
        bundled_name: None,
    })
}

/// Resolve the voice for a run from the layered inputs.
///
/// Precedence (highest first):
/// 1. `cli_value` — `--voice` (a bundled profile name OR a file path).
/// 2. `toml_path` — `[voice].path` (always a file path).
/// 3. `toml_profile` — `[voice].profile` (always a bundled profile name).
/// 4. The bundled `terse` default.
///
/// For `cli_value`, a bundled-name match wins over treating it as a path
/// (so `--voice prose` does what you'd expect even if a `./prose` file
/// happens to exist; pass `./prose` explicitly to bypass).
pub fn resolve(
    cli_value: Option<&str>,
    toml_path: Option<&Path>,
    toml_profile: Option<&str>,
) -> Result<Voice, VoiceError> {
    if let Some(value) = cli_value {
        if bundled_by_name(value).is_some() {
            return load_bundled(value);
        }
        return load_from_file(Path::new(value));
    }
    if let Some(path) = toml_path {
        return load_from_file(path);
    }
    if let Some(name) = toml_profile {
        return load_bundled(name);
    }
    Ok(default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_terse_loads_and_is_marked_not_custom() {
        let v = load_bundled("terse").unwrap();
        assert!(!v.is_custom);
        assert_eq!(v.bundled_name, Some("terse"));
        assert!(!v.system_prompt.is_empty());
        assert!(v.system_prompt.contains("Markdown"));
    }

    #[test]
    fn bundled_prose_loads_and_is_marked_not_custom() {
        let v = load_bundled("prose").unwrap();
        assert!(!v.is_custom);
        assert_eq!(v.bundled_name, Some("prose"));
        // Sentinel phrase that distinguishes prose.md from terse.md.
        assert!(v.system_prompt.contains("Marquee bullets"));
    }

    #[test]
    fn default_alias_resolves_to_terse() {
        let v = load_bundled("default").unwrap();
        assert_eq!(v.bundled_name, Some("terse"));
        assert_eq!(
            v.system_prompt,
            load_bundled("terse").unwrap().system_prompt
        );
    }

    #[test]
    fn default_function_returns_terse() {
        let v = default();
        assert_eq!(v.bundled_name, Some("terse"));
        assert!(!v.is_custom);
    }

    #[test]
    fn load_bundled_unknown_name_errors_with_known_list() {
        let err = load_bundled("operatic").unwrap_err();
        match err {
            VoiceError::UnknownProfile { name, known } => {
                assert_eq!(name, "operatic");
                assert!(known.contains(&"terse"));
                assert!(known.contains(&"prose"));
            }
            other => panic!("expected UnknownProfile, got {other:?}"),
        }
    }

    #[test]
    fn load_from_file_returns_custom_marked_voice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custom.md");
        std::fs::write(&path, "Write release notes like a pirate. Arr.\n").unwrap();
        let v = load_from_file(&path).unwrap();
        assert!(v.is_custom);
        assert_eq!(v.bundled_name, None);
        assert_eq!(v.system_prompt, "Write release notes like a pirate. Arr.");
    }

    #[test]
    fn load_from_file_trims_leading_and_trailing_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("padded.md");
        std::fs::write(&path, "\n\n  Body with padding.  \n\n").unwrap();
        let v = load_from_file(&path).unwrap();
        assert_eq!(v.system_prompt, "Body with padding.");
    }

    #[test]
    fn load_from_file_missing_file_returns_io_err() {
        let err = load_from_file(Path::new("/definitely/not/here.md")).unwrap_err();
        assert!(matches!(err, VoiceError::Io { .. }));
    }

    #[test]
    fn load_from_file_empty_returns_empty_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.md");
        std::fs::write(&path, "   \n\n  \n").unwrap();
        let err = load_from_file(&path).unwrap_err();
        assert!(matches!(err, VoiceError::Empty { .. }));
    }

    #[test]
    fn resolve_no_inputs_returns_terse_default() {
        let v = resolve(None, None, None).unwrap();
        assert_eq!(v.bundled_name, Some("terse"));
    }

    #[test]
    fn resolve_cli_name_loads_bundled() {
        let v = resolve(Some("prose"), None, None).unwrap();
        assert_eq!(v.bundled_name, Some("prose"));
    }

    #[test]
    fn resolve_cli_path_loads_custom_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.md");
        std::fs::write(&path, "custom body").unwrap();
        let v = resolve(Some(path.to_str().unwrap()), None, None).unwrap();
        assert!(v.is_custom);
        assert_eq!(v.system_prompt, "custom body");
    }

    #[test]
    fn resolve_cli_wins_over_toml_path_and_profile() {
        let v = resolve(Some("prose"), Some(Path::new("/nope")), Some("terse")).unwrap();
        assert_eq!(v.bundled_name, Some("prose"));
    }

    #[test]
    fn resolve_toml_path_wins_over_toml_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.md");
        std::fs::write(&path, "custom body").unwrap();
        let v = resolve(None, Some(&path), Some("prose")).unwrap();
        assert!(v.is_custom);
        assert_eq!(v.system_prompt, "custom body");
    }

    #[test]
    fn resolve_toml_profile_loads_bundled() {
        let v = resolve(None, None, Some("prose")).unwrap();
        assert_eq!(v.bundled_name, Some("prose"));
    }

    #[test]
    fn resolve_cli_unknown_name_falls_through_to_path_and_errors() {
        // "nope" isn't a bundled name and isn't a real path → IO error.
        let err = resolve(Some("nope"), None, None).unwrap_err();
        assert!(matches!(err, VoiceError::Io { .. }));
    }
}
