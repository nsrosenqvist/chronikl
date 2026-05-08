//! Terminal progress UI for `chronikl generate`.
//!
//! chronikl's pipeline is sequential (resolve → enrich → ladder →
//! prose → render), so the right shape for the UI is a header block
//! describing the run, one row per stage as it completes, and a
//! summary block at the end. Live spinners aren't worth the
//! complexity for a ~10s pipeline.
//!
//! Output goes to stderr so the rendered Markdown on stdout stays
//! pipe-friendly. Colors are auto-disabled on non-TTY by `colored`.
//! Pass `quiet = true` to suppress everything.
//!
//! All formatting helpers live here so the rest of `main.rs` doesn't
//! repeat the icon/column logic.

use std::fmt;
use std::time::{Duration, Instant};

use colored::Colorize;

use crate::models::TokenUsage;

/// Width of the horizontal rules in the header/summary blocks.
///
/// Sized to span the longest content line — the license banner runs to
/// ~70 chars and the stage rows (`name 22 + detail 32 + duration 8` +
/// padding) land in the same neighborhood, so 70 is the smallest width
/// that doesn't look truncated next to the data.
const RULE_WIDTH: usize = 70;

/// Width of the stage-name column (left-aligned).
const STAGE_NAME_WIDTH: usize = 22;

/// Width of the stage-detail column (left-aligned, between name and
/// duration).
const STAGE_DETAIL_WIDTH: usize = 32;

/// Lightweight progress reporter used throughout `generate_inner`.
///
/// Stateless apart from the start instant — call `stage_start()` to
/// time a stage and feed the returned `Instant` into one of the
/// `stage_*` finishers.
pub struct Progress {
    enabled: bool,
    started: Instant,
}

impl Progress {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            started: Instant::now(),
        }
    }

    /// Total elapsed since the `Progress` was constructed. Used by the
    /// summary block.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Print the header block describing the run context. The
    /// optional `license` line — `LicenseStatus::Licensed { name }` or
    /// `LicenseStatus::Free` — is rendered above the rule with a
    /// green dot, mirroring nitpik's banner.
    pub fn header(&self, info: HeaderInfo<'_>) {
        if !self.enabled {
            return;
        }
        eprintln!("{}", format!("chronikl {}", info.version).bold());
        match info.license {
            LicenseStatus::Licensed { name } => {
                eprintln!(
                    "  {} {}",
                    "●".bright_green(),
                    format!("Licensed to {name}. Thank you for supporting chronikl! ♥").dimmed(),
                );
            }
            LicenseStatus::Free => {
                eprintln!(
                    "  {} {}",
                    "●".bright_green(),
                    "Free for personal & open-source use. Commercial use requires a license."
                        .dimmed(),
                );
            }
        }
        // Single blank line between the banner and the settings — no
        // rule. The next divider (after the settings) is enough to
        // visually section the header off from the stage rows.
        eprintln!();
        self.field("range", info.range);
        self.field("release", info.release);
        self.field("version bump", info.version_bump);
        match (info.provider, info.model) {
            (Some(p), Some(m)) => self.field("provider", &format!("{p} · {m}")),
            (Some(p), None) => self.field("provider", p),
            _ => {}
        }
        self.field("voice", info.voice);
        self.divider();
    }

    fn field(&self, label: &str, value: &str) {
        eprintln!("  {:<14} {value}", label.dimmed());
    }

    /// Plain horizontal rule with a blank line above and below. All
    /// header/summary rules go through this helper so the spacing is
    /// uniform.
    fn divider(&self) {
        if !self.enabled {
            return;
        }
        eprintln!();
        eprintln!("{}", "─".repeat(RULE_WIDTH).dimmed());
        eprintln!();
    }

    /// Mark a stage as started. Returns the start instant so the
    /// caller can pass it back into a `stage_*` finisher.
    pub fn stage_start(&self) -> Instant {
        Instant::now()
    }

    /// Stage completed successfully. `detail` is a short description
    /// of what happened (e.g. `"1 commit in 1 batch"`).
    pub fn stage_done(&self, name: &str, detail: impl fmt::Display, started: Instant) {
        if !self.enabled {
            return;
        }
        let dur = format_duration(started.elapsed());
        eprintln!(
            "  {icon}  {name:<n$} {detail:<d$} {dur:>8}",
            icon = "✓".green(),
            n = STAGE_NAME_WIDTH,
            d = STAGE_DETAIL_WIDTH,
            dur = dur.dimmed(),
        );
    }

    /// Stage was skipped (no work to do, opt-out flag, missing
    /// provider, …). No duration shown.
    pub fn stage_skip(&self, name: &str, reason: impl fmt::Display) {
        if !self.enabled {
            return;
        }
        eprintln!(
            "  {icon}  {name:<n$} {reason}",
            icon = "⊘".dimmed(),
            n = STAGE_NAME_WIDTH,
        );
    }

    /// Stage emitted a non-fatal warning (best-effort fallback fired,
    /// PR enrichment failed for some commits, etc.).
    pub fn stage_warn(&self, name: &str, msg: impl fmt::Display) {
        if !self.enabled {
            return;
        }
        eprintln!(
            "  {icon}  {name:<n$} {msg}",
            icon = "⚠".yellow(),
            n = STAGE_NAME_WIDTH,
        );
    }

    /// Print a centered-label divider to stderr. Used to frame the
    /// rendered Markdown when it gets dumped to stdout — without a
    /// visual marker the document just appears mid-stream between
    /// stage rows and the summary block, which looks broken.
    ///
    /// Always prints to stderr so it never contaminates stdout when
    /// the caller is piping the Markdown out.
    pub fn divider_labeled(&self, label: &str) {
        if !self.enabled {
            return;
        }
        let label_part = format!(" {label} ");
        let label_len = label_part.chars().count();
        let side = RULE_WIDTH.saturating_sub(label_len) / 2;
        let left = "─".repeat(side);
        let right = "─".repeat(RULE_WIDTH.saturating_sub(side + label_len));
        eprintln!();
        eprintln!("{}{}{}", left.dimmed(), label_part.dimmed(), right.dimmed());
        eprintln!();
    }

    /// Free-form info line that doesn't belong to a specific stage
    /// (e.g. cache populate count before any LLM tier runs). Uses the
    /// same `"  icon  text"` indent as the stage rows so the column
    /// where the message text starts is visually aligned with the
    /// stage-name column.
    pub fn info(&self, msg: impl fmt::Display) {
        if !self.enabled {
            return;
        }
        eprintln!("  {}  {msg}", "i".cyan().bold());
    }

    /// Print the summary block at the end of a successful run. When
    /// `info.audit_log_path` is set, a blank-line separated notice is
    /// appended after the totals so the audit-log reference doesn't
    /// blur into the run stats.
    pub fn summary(&self, info: SummaryInfo<'_>) {
        if !self.enabled {
            return;
        }
        self.divider();
        if let Some(path) = info.output_path {
            eprintln!(
                "  {} Wrote {} to {}",
                "✓".green(),
                format_bytes(info.output_bytes).bold(),
                path.bold()
            );
        } else {
            eprintln!(
                "  {} Rendered {}",
                "✓".green(),
                format_bytes(info.output_bytes).bold()
            );
        }
        eprintln!();
        eprintln!(
            "  {}",
            format!(
                "{} commits · {} LLM calls · {} tokens",
                info.commits,
                info.llm_calls,
                format_count(info.tokens.total_tokens),
            )
            .dimmed()
        );
        eprintln!(
            "  {}",
            format!("total: {}", format_duration(info.elapsed)).dimmed()
        );
        if let Some(path) = info.audit_log_path {
            eprintln!();
            eprintln!("  {}  audit log written to {path}", "i".cyan().bold());
        }
    }

    /// Top-level error line, used when `generate_inner` bubbles a
    /// fatal error up to the entrypoint.
    pub fn error(msg: impl fmt::Display) {
        eprintln!("  {}  {msg}", "✗".red().bold());
    }
}

/// Inputs for [`Progress::header`].
#[derive(Default)]
pub struct HeaderInfo<'a> {
    pub version: &'a str,
    pub range: &'a str,
    pub release: &'a str,
    pub version_bump: &'a str,
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
    pub voice: &'a str,
    pub license: LicenseStatus<'a>,
}

/// Whether a valid license key is currently active. Surfaced in the
/// header banner so users know whether they're using chronikl under
/// the free terms or under a commercial license.
#[derive(Default, Debug, Clone)]
pub enum LicenseStatus<'a> {
    #[default]
    Free,
    Licensed {
        name: &'a str,
    },
}

/// Inputs for [`Progress::summary`].
#[derive(Default)]
pub struct SummaryInfo<'a> {
    pub output_path: Option<&'a str>,
    pub output_bytes: usize,
    pub commits: usize,
    pub llm_calls: usize,
    pub tokens: TokenUsage,
    pub elapsed: Duration,
    /// Optional path to the just-written audit log. When set, surfaces
    /// as a separated trailing line below the run totals.
    pub audit_log_path: Option<&'a str>,
}

/// Format a duration as `"123ms"` / `"1.2s"` / `"1m 03s"`. Sub-second
/// values keep their `ms` granularity to feel snappy; multi-minute
/// runs switch to `"Xm YYs"` for readability.
pub fn format_duration(d: Duration) -> String {
    let total_ms = d.as_millis();
    if total_ms < 1000 {
        return format!("{total_ms}ms");
    }
    let secs = d.as_secs();
    if secs < 60 {
        return format!("{:.1}s", d.as_secs_f32());
    }
    let mins = secs / 60;
    let rem = secs % 60;
    format!("{mins}m {rem:02}s")
}

/// Format a byte count as `"123 B"`, `"4.5 KB"`, `"1.2 MB"`.
pub fn format_bytes(n: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let n = n as f64;
    if n < KB {
        format!("{} B", n as usize)
    } else if n < MB {
        format!("{:.1} KB", n / KB)
    } else {
        format!("{:.1} MB", n / MB)
    }
}

/// Format an integer with thousands separators (`1234567` →
/// `"1,234,567"`). Used for token counts in the summary so big
/// numbers stay scannable.
pub fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_formatting_chooses_unit() {
        assert_eq!(format_duration(Duration::from_millis(0)), "0ms");
        assert_eq!(format_duration(Duration::from_millis(123)), "123ms");
        assert_eq!(format_duration(Duration::from_millis(1234)), "1.2s");
        assert_eq!(format_duration(Duration::from_secs(30)), "30.0s");
        assert_eq!(format_duration(Duration::from_secs(75)), "1m 15s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 05s");
    }

    #[test]
    fn bytes_formatting_chooses_unit() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
    }

    #[test]
    fn count_formatting_inserts_thousands_separators() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_234), "1,234");
        assert_eq!(format_count(1_234_567), "1,234,567");
    }

    /// Reproduces the geometry of `divider_labeled` in pure form so we
    /// can assert the centering math without capturing stderr. Failure
    /// here means the live divider would also be mis-centered.
    fn labeled_divider(label: &str, total: usize) -> (usize, usize) {
        let label_part = format!(" {label} ");
        let label_len = label_part.chars().count();
        let side = total.saturating_sub(label_len) / 2;
        let right = total.saturating_sub(side + label_len);
        (side, right)
    }

    #[test]
    fn divider_label_is_centered() {
        let (l, r) = labeled_divider("release notes", RULE_WIDTH);
        // " release notes " = 15 chars; sides should sum to RULE_WIDTH - 15.
        assert_eq!(l + r + 15, RULE_WIDTH);
        // Centered split: 27 / 28 at width 70 (off by one for odd-len label).
        assert!(l == 27 && r == 28, "expected 27/28, got {l}/{r}");
    }

    #[test]
    fn divider_handles_oversized_label_gracefully() {
        // Label longer than the total rule width — math must not panic.
        let (l, r) = labeled_divider("a-very-very-long-label-that-exceeds", 20);
        assert_eq!(l, 0);
        assert_eq!(r, 0);
    }
}
