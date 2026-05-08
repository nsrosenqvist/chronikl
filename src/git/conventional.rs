//! Conventional Commits parser.
//!
//! Spec: <https://www.conventionalcommits.org/>
//!
//! Parses the first line of a commit message into a [`Conventional`]
//! struct, and detects breaking-change markers (`!` after the type or a
//! `BREAKING CHANGE:` footer in the body).

use std::sync::OnceLock;

use regex::Regex;

use crate::models::Conventional;

/// Match `type(scope)?!?: description`.
///
/// `type` and `scope` use a permissive character class so we accept
/// real-world variants like `wip` or `feat-2`. We lowercase before
/// comparison so case doesn't matter at the call site.
fn header_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?P<kind>[A-Za-z][A-Za-z0-9_-]*)(?:\((?P<scope>[^)]+)\))?(?P<bang>!)?:\s+(?P<desc>.+)$")
            .expect("static regex compiles")
    })
}

fn breaking_footer_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Footer can be `BREAKING CHANGE:` or `BREAKING-CHANGE:` per spec.
    RE.get_or_init(|| Regex::new(r"(?m)^BREAKING[ -]CHANGE:").expect("static regex compiles"))
}

/// Parse a commit message into a Conventional Commits classification.
///
/// `subject` is the first line of the message. `body` is everything after,
/// scanned for a `BREAKING CHANGE:` footer.
///
/// Returns `None` if the subject doesn't match the Conventional Commits
/// header form.
pub fn parse(subject: &str, body: &str) -> Option<Conventional> {
    let caps = header_re().captures(subject.trim())?;
    let kind = caps.name("kind")?.as_str().to_ascii_lowercase();
    let scope = caps.name("scope").map(|m| m.as_str().trim().to_string());
    let bang = caps.name("bang").is_some();
    let description = caps.name("desc")?.as_str().trim().to_string();

    let footer_breaking = breaking_footer_re().is_match(body);

    Some(Conventional {
        kind,
        scope,
        breaking: bang || footer_breaking,
        description,
    })
}

/// Standalone breaking-change detection for non-Conventional commits.
///
/// A subject without a Conventional header may still declare a breaking
/// change in the body footer.
pub fn has_breaking_footer(body: &str) -> bool {
    breaking_footer_re().is_match(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_feat() {
        let c = parse("feat: add login flow", "").unwrap();
        assert_eq!(c.kind, "feat");
        assert_eq!(c.scope, None);
        assert!(!c.breaking);
        assert_eq!(c.description, "add login flow");
    }

    #[test]
    fn parses_scope() {
        let c = parse("fix(parser): handle empty input", "").unwrap();
        assert_eq!(c.kind, "fix");
        assert_eq!(c.scope.as_deref(), Some("parser"));
    }

    #[test]
    fn detects_bang_breaking() {
        let c = parse("feat!: drop Node 18 support", "").unwrap();
        assert!(c.breaking);
    }

    #[test]
    fn detects_scope_with_bang() {
        let c = parse("chore(deps)!: bump major versions", "").unwrap();
        assert_eq!(c.kind, "chore");
        assert_eq!(c.scope.as_deref(), Some("deps"));
        assert!(c.breaking);
    }

    #[test]
    fn detects_breaking_footer() {
        let c = parse(
            "feat: add v2 endpoint",
            "Adds /v2/users.\n\nBREAKING CHANGE: /v1/users is removed.",
        )
        .unwrap();
        assert!(c.breaking);
    }

    #[test]
    fn detects_breaking_footer_with_hyphen() {
        let c = parse("feat: x", "BREAKING-CHANGE: y").unwrap();
        assert!(c.breaking);
    }

    #[test]
    fn lowercase_kind() {
        let c = parse("FEAT: add thing", "").unwrap();
        assert_eq!(c.kind, "feat");
    }

    #[test]
    fn rejects_missing_colon() {
        assert!(parse("feat add login flow", "").is_none());
    }

    #[test]
    fn rejects_no_description() {
        assert!(parse("feat:", "").is_none());
    }

    #[test]
    fn rejects_random_subject() {
        assert!(parse("WIP", "").is_none());
        assert!(parse("Merge pull request #42", "").is_none());
        assert!(parse("Update README", "").is_none());
    }

    #[test]
    fn has_breaking_footer_works_alone() {
        assert!(has_breaking_footer("BREAKING CHANGE: yes"));
        assert!(has_breaking_footer("BREAKING-CHANGE: yes"));
        assert!(!has_breaking_footer("regular text"));
    }
}
