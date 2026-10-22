//! Final shaping of the release-notes text.
//!
//! Whatever produced the body — the prose model or the deterministic
//! renderer — this pass makes the output match the run's
//! [`RenderSettings`]: flat list or sections, plain text or Markdown,
//! footer on or off, and the bullet/character limits. The prompts ask
//! the model for the same shape; shaping here guarantees it, so a CI
//! caller never has to post-process chronikl's output.
//!
//! Limits cut at whole bullets, from the end, so the model's ranking
//! (most noticeable first) decides what stays. A bullet is never cut
//! mid-text.

use std::sync::LazyLock;

use regex::Regex;

use crate::models::{RenderSettings, TextFormat};

/// Result of [`shape`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shaped {
    /// The final text, ending in one line break (empty when nothing
    /// fit).
    pub text: String,
    /// Bullets removed to meet `max_bullets` / `max_chars`.
    pub dropped_bullets: usize,
    /// `false` when even the remaining text exceeds `max_chars` (only
    /// possible when nothing could be removed without leaving nothing).
    pub fits: bool,
}

/// One block of the parsed document.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Block {
    /// A heading's text, without its `#` marks.
    Heading(String),
    /// A list item: its first line (with its marker) plus any indented
    /// continuation lines and nested items.
    Bullet(Vec<String>),
    /// A paragraph's lines.
    Paragraph(Vec<String>),
    /// A trailer such as `**Full Changelog**: <url>`.
    Trailer(String),
}

static BULLET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ ?(?:[-*+•]|\d{1,3}[.)])\s+\S").expect("static regex"));
static HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^#{1,6}\s+(.*?)\s*#*\s*$").expect("static regex"));
static TRAILER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(\*\*|__)?full changelog(\*\*|__)?\s*:").expect("static regex")
});

/// Shape `body` for output and append `footer` (when `Some` and
/// `render.footer` is on).
pub fn shape(body: &str, footer: Option<&str>, render: &RenderSettings) -> Shaped {
    let footer = footer.filter(|_| render.footer);

    // Default shape: leave the body as written, apart from the footer.
    // Keeps chronikl's long-standing output byte-for-byte.
    if !render.changes_shape() {
        let text = if render.footer {
            append_footer(body, footer)
        } else {
            finish(
                body.lines()
                    .filter(|l| !TRAILER_RE.is_match(l.trim()))
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim(),
            )
        };
        return Shaped {
            text,
            dropped_bullets: 0,
            fits: true,
        };
    }

    let mut blocks = parse(body);
    let has_bullets = blocks.iter().any(|b| matches!(b, Block::Bullet(_)));

    if !render.headings && has_bullets {
        blocks.retain(|b| matches!(b, Block::Bullet(_) | Block::Trailer(_)));
    }
    if !render.footer {
        blocks.retain(|b| !matches!(b, Block::Trailer(_)));
    }
    if let Some(footer) = footer {
        let present = blocks
            .iter()
            .any(|b| matches!(b, Block::Trailer(t) if t.contains(footer)));
        if !present {
            blocks.push(Block::Trailer(footer.to_string()));
        }
    }
    if render.format == TextFormat::Plain {
        blocks = blocks.into_iter().map(to_plain_block).collect();
    }

    let mut dropped = 0usize;
    if let Some(max) = render.max_bullets {
        let mut seen = 0usize;
        blocks.retain(|b| {
            if matches!(b, Block::Bullet(_)) {
                seen += 1;
                if seen > max {
                    dropped += 1;
                    return false;
                }
            }
            true
        });
    }
    let mut fits = true;
    if let Some(max) = render.max_chars {
        // Trailers go first, then bullets from the end, then any other
        // blocks from the end.
        while char_len(&render_blocks(&blocks, render)) > max {
            if let Some(i) = blocks.iter().rposition(|b| matches!(b, Block::Trailer(_))) {
                blocks.remove(i);
            } else if let Some(i) = blocks.iter().rposition(|b| matches!(b, Block::Bullet(_))) {
                blocks.remove(i);
                dropped += 1;
            } else if !blocks.is_empty() {
                blocks.pop();
            } else {
                fits = false;
                break;
            }
            prune_empty_headings(&mut blocks);
        }
    }
    prune_empty_headings(&mut blocks);

    Shaped {
        text: finish(&render_blocks(&blocks, render)),
        dropped_bullets: dropped,
        fits,
    }
}

/// Append a footer line to a document, separated by a blank line.
/// Idempotent when the footer is already present — guards against the
/// prose model occasionally including the changelog link itself.
pub fn append_footer(markdown: &str, footer: Option<&str>) -> String {
    let trimmed = markdown.trim_end();
    match footer {
        Some(footer) if !trimmed.contains(footer) => {
            if trimmed.is_empty() {
                format!("{footer}\n")
            } else {
                format!("{trimmed}\n\n{footer}\n")
            }
        }
        _ => finish(trimmed),
    }
}

fn finish(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

fn char_len(s: &str) -> usize {
    s.trim().chars().count()
}

fn parse(body: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut in_fence = false;
    // Whether the previous line was blank (ends a paragraph; an
    // indented line after a blank still continues a bullet).
    let mut after_blank = false;

    for raw in body.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            push_paragraph_line(&mut blocks, line, after_blank);
            after_blank = false;
            continue;
        }
        if in_fence {
            push_paragraph_line(&mut blocks, line, false);
            continue;
        }
        if trimmed.is_empty() {
            after_blank = true;
            continue;
        }
        let indented = line.len() - trimmed.len() >= 2;
        if indented && let Some(Block::Bullet(lines)) = blocks.last_mut() {
            lines.push(line.to_string());
            after_blank = false;
            continue;
        }
        if TRAILER_RE.is_match(trimmed) {
            blocks.push(Block::Trailer(trimmed.to_string()));
        } else if let Some(caps) = HEADING_RE.captures(trimmed) {
            blocks.push(Block::Heading(caps[1].to_string()));
        } else if BULLET_RE.is_match(line) {
            blocks.push(Block::Bullet(vec![trimmed.to_string()]));
        } else if !after_blank && let Some(Block::Bullet(lines)) = blocks.last_mut() {
            // A lazy continuation line of a wrapped bullet.
            lines.push(format!("  {trimmed}"));
        } else {
            push_paragraph_line(&mut blocks, trimmed, after_blank);
        }
        after_blank = false;
    }
    blocks
}

fn push_paragraph_line(blocks: &mut Vec<Block>, line: &str, new_paragraph: bool) {
    match blocks.last_mut() {
        Some(Block::Paragraph(lines)) if !new_paragraph => lines.push(line.to_string()),
        _ => blocks.push(Block::Paragraph(vec![line.to_string()])),
    }
}

/// Remove headings whose section (up to the next heading or trailer)
/// has nothing left in it.
fn prune_empty_headings(blocks: &mut Vec<Block>) {
    let mut keep = vec![true; blocks.len()];
    for (i, block) in blocks.iter().enumerate() {
        if matches!(block, Block::Heading(_)) {
            let has_content = blocks[i + 1..]
                .iter()
                .take_while(|b| !matches!(b, Block::Heading(_) | Block::Trailer(_)))
                .next()
                .is_some();
            keep[i] = has_content;
        }
    }
    let mut it = keep.into_iter();
    blocks.retain(|_| it.next().unwrap_or(true));
}

fn render_blocks(blocks: &[Block], render: &RenderSettings) -> String {
    let mut out = String::new();
    let mut prev: Option<&Block> = None;
    for block in blocks {
        if let Some(p) = prev {
            let tight = matches!((p, block), (Block::Bullet(_), Block::Bullet(_)));
            out.push_str(if tight { "\n" } else { "\n\n" });
        }
        match block {
            Block::Heading(text) => match render.format {
                TextFormat::Markdown => {
                    out.push_str("### ");
                    out.push_str(text);
                }
                TextFormat::Plain => out.push_str(text),
            },
            Block::Bullet(lines) | Block::Paragraph(lines) => out.push_str(&lines.join("\n")),
            Block::Trailer(text) => out.push_str(text),
        }
        prev = Some(block);
    }
    out
}

static BOLD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\*\*(.+?)\*\*|__(.+?)__").expect("static regex"));
static ITALIC_STAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[^\w*])\*([^*\s](?:[^*]*[^*\s])?)\*").expect("static regex"));
static ITALIC_UNDERSCORE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[^\w])_([^_\s](?:[^_]*[^_\s])?)_($|[^\w])").expect("static regex")
});
static CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`+([^`]+)`+").expect("static regex"));
static LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!?\[([^\]]*)\]\([^)]*\)").expect("static regex"));
static MARKER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)(?:[*+•]|-)\s+").expect("static regex"));

/// Strip Markdown from one line: emphasis, code spans, links, quote
/// marks; normalize bullet markers to `- `.
pub fn plain_line(line: &str) -> String {
    let mut s = line.to_string();
    if let Some(rest) = s.trim_start().strip_prefix("> ") {
        s = rest.to_string();
    }
    s = LINK_RE.replace_all(&s, "$1").into_owned();
    s = CODE_RE.replace_all(&s, "$1").into_owned();
    s = BOLD_RE.replace_all(&s, "$1$2").into_owned();
    s = ITALIC_STAR_RE.replace_all(&s, "$1$2").into_owned();
    s = ITALIC_UNDERSCORE_RE.replace_all(&s, "$1$2$3").into_owned();
    MARKER_RE.replace(&s, "$1- ").into_owned()
}

fn to_plain_block(block: Block) -> Block {
    match block {
        Block::Heading(text) => Block::Heading(plain_line(&text)),
        Block::Bullet(lines) => Block::Bullet(lines.iter().map(|l| plain_line(l)).collect()),
        Block::Paragraph(lines) => Block::Paragraph(
            lines
                .iter()
                .filter(|l| !l.trim_start().starts_with("```"))
                .map(|l| plain_line(l))
                .collect(),
        ),
        Block::Trailer(text) => Block::Trailer(plain_line(&text)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECTIONED: &str = "This release adds search and fixes two crashes.\n\n\
### Features\n\n\
- **Search** — find places by name\n\
- Add dark mode\n\n\
### Bug Fixes\n\n\
- Fix a crash on start\n\
- Fix sync after sleep\n\n\
### Tests\n\n\
- Add tests for sign-in limits\n\n\
**Full Changelog**: https://example.com/compare/a...b";

    fn flat() -> RenderSettings {
        RenderSettings {
            headings: false,
            footer: false,
            ..Default::default()
        }
    }

    #[test]
    fn default_shape_keeps_body_and_appends_footer() {
        let out = shape(
            "### Features\n\n- a\n",
            Some("**Full Changelog**: x"),
            &RenderSettings::default(),
        );
        assert_eq!(out.text, "### Features\n\n- a\n\n**Full Changelog**: x\n");
        assert_eq!(out.dropped_bullets, 0);
    }

    #[test]
    fn default_shape_does_not_duplicate_footer() {
        let out = shape(
            "- a\n\n**Full Changelog**: x",
            Some("**Full Changelog**: x"),
            &RenderSettings::default(),
        );
        assert_eq!(out.text.matches("Full Changelog").count(), 1);
    }

    #[test]
    fn footer_off_drops_model_written_changelog_line() {
        let render = RenderSettings {
            footer: false,
            ..Default::default()
        };
        let out = shape(SECTIONED, Some("**Full Changelog**: y"), &render);
        assert!(!out.text.contains("Full Changelog"), "{}", out.text);
        assert!(out.text.contains("### Features"));
    }

    #[test]
    fn flat_rendering_keeps_only_bullets_in_order() {
        let out = shape(SECTIONED, None, &flat());
        assert_eq!(
            out.text,
            "- **Search** — find places by name\n- Add dark mode\n- Fix a crash on start\n\
- Fix sync after sleep\n- Add tests for sign-in limits\n"
        );
    }

    #[test]
    fn flat_rendering_keeps_text_when_there_are_no_bullets() {
        let out = shape("Just a paragraph.\n", None, &flat());
        assert_eq!(out.text, "Just a paragraph.\n");
    }

    #[test]
    fn max_bullets_cuts_whole_bullets_from_the_end() {
        let render = RenderSettings {
            max_bullets: Some(2),
            ..flat()
        };
        let out = shape(SECTIONED, None, &render);
        assert_eq!(
            out.text,
            "- **Search** — find places by name\n- Add dark mode\n"
        );
        assert_eq!(out.dropped_bullets, 3);
    }

    #[test]
    fn max_bullets_with_headings_drops_emptied_sections() {
        let render = RenderSettings {
            max_bullets: Some(3),
            footer: false,
            ..Default::default()
        };
        let out = shape(SECTIONED, None, &render);
        assert!(out.text.contains("### Bug Fixes\n\n- Fix a crash on start"));
        assert!(!out.text.contains("Fix sync"));
        assert!(!out.text.contains("### Tests"), "{}", out.text);
        assert!(out.text.starts_with("This release adds search"));
    }

    #[test]
    fn max_chars_cuts_at_bullet_boundaries() {
        let body = "- one one one\n- two two two\n- three three three\n";
        // "- one one one\n- two two two" is 27 chars.
        let render = RenderSettings {
            max_chars: Some(30),
            ..flat()
        };
        let out = shape(body, None, &render);
        assert_eq!(out.text, "- one one one\n- two two two\n");
        assert_eq!(out.dropped_bullets, 1);
        assert!(out.fits);
        assert!(out.text.trim().chars().count() <= 30);
    }

    #[test]
    fn max_chars_never_cuts_mid_bullet() {
        let render = RenderSettings {
            max_chars: Some(5),
            ..flat()
        };
        let out = shape("- a long bullet\n", None, &render);
        assert_eq!(out.text, "");
        assert_eq!(out.dropped_bullets, 1);
    }

    #[test]
    fn max_chars_drops_the_footer_before_bullets() {
        let render = RenderSettings {
            max_chars: Some(12),
            headings: false,
            ..Default::default()
        };
        let out = shape("- a\n- b\n", Some("**Full Changelog**: https://x"), &render);
        assert_eq!(out.text, "- a\n- b\n");
    }

    #[test]
    fn max_chars_counts_unicode_characters() {
        let render = RenderSettings {
            max_chars: Some(6),
            ..flat()
        };
        let out = shape("- åäö\n- ü\n", None, &render);
        assert_eq!(out.text, "- åäö\n");
    }

    #[test]
    fn continuation_lines_stay_with_their_bullet() {
        let body = "- first\n  more of first\n  - nested\n- second\n";
        let render = RenderSettings {
            max_bullets: Some(1),
            ..flat()
        };
        let out = shape(body, None, &render);
        assert_eq!(out.text, "- first\n  more of first\n  - nested\n");
    }

    #[test]
    fn plain_format_strips_markdown() {
        let render = RenderSettings {
            format: TextFormat::Plain,
            ..flat()
        };
        let body =
            "* **Search** finds `places` by [name](https://x)\n- _Faster_ start, snake_case kept\n";
        let out = shape(body, None, &render);
        assert_eq!(
            out.text,
            "- Search finds places by name\n- Faster start, snake_case kept\n"
        );
    }

    #[test]
    fn plain_format_with_headings_writes_bare_section_names() {
        let render = RenderSettings {
            format: TextFormat::Plain,
            footer: false,
            ..Default::default()
        };
        let out = shape("### Features\n\n- **A** thing\n", None, &render);
        assert_eq!(out.text, "Features\n\n- A thing\n");
    }

    #[test]
    fn end_user_like_settings_produce_store_text() {
        let render = RenderSettings {
            user_facing: true,
            headings: false,
            footer: false,
            max_bullets: Some(5),
            max_chars: Some(80),
            format: TextFormat::Plain,
            ..Default::default()
        };
        let out = shape(SECTIONED, Some("**Full Changelog**: z"), &render);
        assert!(!out.text.contains('#'));
        assert!(!out.text.contains("**"));
        assert!(!out.text.contains("Full Changelog"));
        assert!(out.text.trim().chars().count() <= 80);
        assert!(out.text.starts_with("- Search — find places by name\n"));
    }

    #[test]
    fn plain_line_handles_quotes_and_images() {
        assert_eq!(plain_line("> ![logo](x.png) hi"), "logo hi");
        assert_eq!(plain_line("  + nested"), "  - nested");
        assert_eq!(plain_line("1. first"), "1. first");
    }
}
