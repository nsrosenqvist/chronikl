//! Voice-file frontmatter: render defaults a voice carries with it.
//!
//! A voice file may start with a frontmatter block between `---` lines.
//! It holds the same keys as `[render]` in `.chronikl.toml`, written as
//! YAML-style `key: value` lines:
//!
//! ```text
//! ---
//! headings: false
//! footer: false
//! exclude_sections: [docs, tests, ci]
//! max_chars: 450
//! fallback_other: "Small improvements behind the scenes."
//! ---
//! You write the "What's new" text for …
//! ```
//!
//! Only flat keys are supported: booleans, whole numbers, strings
//! (quoted or bare) and lists of strings (`[a, b]` on one line, or one
//! `- item` per line below the key). Comments start with `#`. Unknown
//! keys are an error, so a typo doesn't silently do nothing.

use serde_json::{Map, Value};

use crate::config::RenderConfig;

/// Split `raw` into its frontmatter (without the `---` lines) and body.
/// Returns `None` for the frontmatter when the file doesn't start with
/// a `---` line or the block is never closed.
pub fn split(raw: &str) -> (Option<&str>, &str) {
    let text = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return (None, raw);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return (Some(&rest[..offset]), &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    (None, raw)
}

/// Parse a frontmatter block into render defaults.
pub fn parse(block: &str) -> Result<RenderConfig, String> {
    let mut map = Map::new();
    let mut open_list: Option<String> = None;

    for (n, raw_line) in block.lines().enumerate() {
        let line = strip_comment(raw_line);
        if line.trim().is_empty() {
            continue;
        }
        let lineno = n + 2; // the opening `---` is line 1
        if let Some(item) = line.trim_start().strip_prefix("- ") {
            let Some(key) = &open_list else {
                return Err(format!("line {lineno}: list item without a key"));
            };
            if let Some(Value::Array(items)) = map.get_mut(key) {
                items.push(Value::String(unquote(item.trim()).to_string()));
            }
            continue;
        }
        open_list = None;
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("line {lineno}: expected `key: value`"));
        };
        let key = key.trim().to_string();
        let value = value.trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            return Err(format!("line {lineno}: invalid key `{key}`"));
        }
        let parsed = if value.is_empty() {
            open_list = Some(key.clone());
            Value::Array(Vec::new())
        } else {
            scalar_or_list(value)
        };
        if map.insert(key.clone(), parsed).is_some() {
            return Err(format!("line {lineno}: `{key}` is set twice"));
        }
    }

    serde_json::from_value(Value::Object(map)).map_err(|e| e.to_string())
}

fn strip_comment(line: &str) -> &str {
    // A `#` starts a comment unless it's inside quotes.
    let mut quote: Option<char> = None;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, '#') if i == 0 || line[..i].ends_with(char::is_whitespace) => {
                return &line[..i];
            }
            _ => {}
        }
    }
    line
}

fn unquote(s: &str) -> &str {
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return &s[1..s.len() - 1];
        }
    }
    s
}

fn scalar_or_list(value: &str) -> Value {
    if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        return Value::Array(
            inner
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| Value::String(unquote(s).to_string()))
                .collect(),
        );
    }
    let quoted = unquote(value);
    if quoted.len() != value.len() {
        return Value::String(quoted.to_string());
    }
    match value {
        "true" | "yes" | "on" => Value::Bool(true),
        "false" | "no" | "off" => Value::Bool(false),
        _ => match value.parse::<u64>() {
            Ok(n) => Value::from(n),
            Err(_) => Value::String(value.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TextFormat;

    #[test]
    fn split_separates_frontmatter_and_body() {
        let (fm, body) = split("---\nheadings: false\n---\nBody text.\n");
        assert_eq!(fm, Some("headings: false\n"));
        assert_eq!(body, "Body text.\n");
    }

    #[test]
    fn split_without_frontmatter_returns_everything() {
        assert_eq!(split("Just a body.\n"), (None, "Just a body.\n"));
        // An unclosed block isn't frontmatter.
        let raw = "---\nheadings: false\nno end";
        assert_eq!(split(raw), (None, raw));
        // A horizontal rule later in the file isn't frontmatter either.
        let raw = "Body\n---\nmore";
        assert_eq!(split(raw), (None, raw));
    }

    #[test]
    fn parses_every_value_shape() {
        let fm = "\
# Render defaults for app-store notes
headings: false
footer: no
max_bullets: 5
max_chars: 450   # Google Play allows 500
format: plain
user_facing: true
exclude_sections: [docs, tests, \"ci\"]
include_sections:
  - features
  - fixes
fallback_line: \"Small fixes # and polish.\"
fallback_fixes: Bug fixes and stability improvements.
";
        let r = parse(fm).unwrap();
        assert_eq!(r.headings, Some(false));
        assert_eq!(r.footer, Some(false));
        assert_eq!(r.max_bullets, Some(5));
        assert_eq!(r.max_chars, Some(450));
        assert_eq!(r.format, Some(TextFormat::Plain));
        assert_eq!(r.user_facing, Some(true));
        assert_eq!(
            r.exclude_sections,
            Some(vec!["docs".into(), "tests".into(), "ci".into()])
        );
        assert_eq!(
            r.include_sections,
            Some(vec!["features".into(), "fixes".into()])
        );
        assert_eq!(
            r.fallback_line.as_deref(),
            Some("Small fixes # and polish.")
        );
        assert_eq!(
            r.fallback_fixes.as_deref(),
            Some("Bug fixes and stability improvements.")
        );
    }

    #[test]
    fn rejects_unknown_keys_and_bad_values() {
        assert!(parse("heading: false").unwrap_err().contains("heading"));
        assert!(parse("max_chars: lots").is_err());
        assert!(parse("format: html").is_err());
        assert!(parse("just text").unwrap_err().contains("key: value"));
        assert!(
            parse("footer: true\nfooter: false")
                .unwrap_err()
                .contains("twice")
        );
        assert!(parse("- orphan").is_err());
    }

    #[test]
    fn empty_frontmatter_sets_nothing() {
        assert_eq!(parse("").unwrap(), RenderConfig::default());
    }
}
