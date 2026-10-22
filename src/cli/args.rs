//! clap-derive CLI definitions.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::constants::FULL_VERSION;
use crate::models::{Temperature, TextFormat};

#[derive(Parser, Debug)]
#[command(
    name = "chronikl",
    version = FULL_VERSION,
    about = "AI-powered release notes generator",
    long_about = None
)]
pub struct Cli {
    /// Default-mode args (used when no subcommand is given). Generate
    /// release notes for the chosen range.
    #[command(flatten)]
    pub generate: GenerateArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Args for the default "generate notes" invocation (`chronikl [OPTIONS]`).
#[derive(Args, Debug, Clone, Default)]
pub struct GenerateArgs {
    #[command(flatten)]
    pub range: RangeArgs,

    /// Repository path. Defaults to the current working directory.
    #[arg(long, value_name = "PATH")]
    pub repo: Option<PathBuf>,

    /// Write Markdown to this path instead of stdout.
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Skip all LLM calls (Tier 1+ and the prose pass). Tier 0
    /// deterministic classification + Markdown rendering still run.
    /// Useful for local previews when a provider isn't set up.
    #[arg(long)]
    pub no_llm: bool,

    /// Skip PR enrichment even when a GitHub remote and token are
    /// detected. Useful for offline runs and tests.
    #[arg(long, env = "CHRONIKL_NO_PR_ENRICHMENT")]
    pub no_pr_enrichment: bool,

    /// Voice profile name (`terse`, `prose`, `simple`) or path to a custom
    /// voice markdown file. Bundled-profile names win over treating the
    /// value as a path. Overrides `voice.path` / `voice.profile` from
    /// TOML. When unset, the bundled `terse` voice is used.
    #[arg(long, value_name = "NAME_OR_FILE")]
    pub voice: Option<String>,

    /// One-shot system-prompt addendum, appended after the voice and
    /// any TOML `voice.extra_instructions`.
    #[arg(long, value_name = "TEXT")]
    pub prompt: Option<String>,

    /// Include commit bodies and PR bodies in the prose-pass user
    /// prompt. Off by default — pairs well with `--voice prose` when
    /// you want the model to write multi-sentence explanations. Also
    /// settable via `[voice].rich_context = true` in TOML.
    #[arg(long)]
    pub rich_context: bool,

    /// One-line project description folded into the prose-pass user
    /// prompt. Overrides auto-detection from `Cargo.toml` /
    /// `package.json` / `pyproject.toml` and the `[project].description`
    /// field in TOML.
    #[arg(long, value_name = "TEXT")]
    pub project_description: Option<String>,

    /// Path to a README file whose intro is folded into the prose-pass
    /// user prompt. Defaults to auto-detection (`README.md`,
    /// `README.rst`, `README.txt`, `README`) in the repo root.
    #[arg(long, value_name = "PATH", conflicts_with = "no_readme")]
    pub readme: Option<PathBuf>,

    /// Disable README detection entirely. Use when the README contains
    /// content that shouldn't go to the LLM.
    #[arg(long, conflicts_with = "readme")]
    pub no_readme: bool,

    /// Enable Tier 3 agentic fallback. The model gets read-only tools
    /// (read_file, list_directory, search_text) so it can explore the
    /// repo to figure out commits the lower tiers can't classify
    /// confidently. Off by default; opt-in because it's the most
    /// expensive tier.
    #[arg(long)]
    pub agent: bool,

    /// Skip the on-disk classification cache. Forces every commit to be
    /// re-classified from scratch.
    #[arg(long)]
    pub no_cache: bool,

    /// Write a JSON audit log of every LLM call (model, tokens, prompt
    /// hash, response hash, commits covered) to this path. Opt-in.
    #[arg(long, value_name = "FILE", env = "CHRONIKL_AUDIT_LOG")]
    pub audit_log: Option<PathBuf>,

    #[command(flatten)]
    pub render: RenderArgs,

    /// Sampling temperature for every LLM call: a number (0 to 2), or
    /// `none` to send no temperature at all. Defaults to 0. A model
    /// that rejects the parameter gets the call retried without it.
    #[arg(long, value_name = "N|none")]
    pub temperature: Option<Temperature>,

    /// Exit non-zero, writing nothing, when an LLM pass fails or no
    /// provider is usable and the notes would otherwise fall back to
    /// the deterministic renderer. Ignored with `--no-llm`.
    #[arg(long)]
    pub strict: bool,
}

/// Rendering flags, shared by the main command and `debug prompts` so
/// the dumped prompts match what a run would send.
#[derive(Args, Debug, Clone, Default)]
pub struct RenderArgs {
    /// Section headings on or off. `--headings=false` renders one flat
    /// bullet list.
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    pub headings: Option<bool>,

    /// Keep only these sections (comma-separated ids, e.g.
    /// `features,fixes`).
    #[arg(long, value_name = "IDS", value_delimiter = ',')]
    pub include_sections: Option<Vec<String>>,

    /// Drop these sections before the prose pass (comma-separated ids,
    /// e.g. `tests,docs,ci`).
    #[arg(long, value_name = "IDS", value_delimiter = ',')]
    pub exclude_sections: Option<Vec<String>>,

    /// Keep at most this many bullets, cut at whole bullets in output
    /// order.
    #[arg(long, value_name = "N")]
    pub max_bullets: Option<usize>,

    /// Keep the whole output within this many characters, cut at whole
    /// bullets.
    #[arg(long, value_name = "N")]
    pub max_chars: Option<usize>,

    /// The "Full Changelog" compare-link footer on or off.
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    pub footer: Option<bool>,

    /// Output text format: `markdown` (default) or `plain`.
    #[arg(long, value_name = "FORMAT")]
    pub format: Option<TextFormat>,

    /// Notes for the product's users: keep only changes they'd notice,
    /// and write a fallback line when nothing is user-facing. The
    /// `simple` voice turns this on.
    #[arg(long, value_name = "BOOL", num_args = 0..=1, default_missing_value = "true")]
    pub user_facing: Option<bool>,

    /// "Nothing user-facing" line for any range (overrides the built-in
    /// lines; a per-kind line below wins over it).
    #[arg(long, value_name = "TEXT")]
    pub fallback_line: Option<String>,

    /// "Nothing user-facing" line for a range with bug fixes.
    #[arg(long, value_name = "TEXT")]
    pub fallback_fixes: Option<String>,

    /// "Nothing user-facing" line for a range with performance work.
    #[arg(long, value_name = "TEXT")]
    pub fallback_performance: Option<String>,

    /// "Nothing user-facing" line for any other range.
    #[arg(long, value_name = "TEXT")]
    pub fallback_other: Option<String>,
}

impl RenderArgs {
    /// Fold these flags over the `[render]` config (flags win).
    pub fn apply_to(&self, render: &mut crate::config::RenderConfig) {
        render.overlay(crate::config::RenderConfig {
            headings: self.headings,
            include_sections: self.include_sections.clone(),
            exclude_sections: self.exclude_sections.clone(),
            max_bullets: self.max_bullets,
            max_chars: self.max_chars,
            footer: self.footer,
            format: self.format,
            user_facing: self.user_facing,
            fallback_line: self.fallback_line.clone(),
            fallback_fixes: self.fallback_fixes.clone(),
            fallback_performance: self.fallback_performance.clone(),
            fallback_other: self.fallback_other.clone(),
        });
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Print version and build metadata.
    Version,

    /// Validate a voice file (plain Markdown — must be readable, non-empty).
    Validate {
        #[arg(value_name = "FILE")]
        file: PathBuf,
    },

    /// Manage the on-disk classification cache.
    #[command(subcommand)]
    Cache(CacheCommand),

    /// Self-update the chronikl binary from the latest GitHub release.
    Update {
        /// Re-install even when the running version equals the latest.
        #[arg(long)]
        force: bool,
    },

    /// Diagnostic helpers (subject to change without notice).
    #[command(subcommand)]
    Debug(DebugCommand),
}

#[derive(Subcommand, Debug)]
pub enum CacheCommand {
    /// Print the cache root for the current schema.
    Path,
    /// Print the entry count and total size.
    Stats,
    /// Remove every cached classification at the current schema.
    Clear,
}

#[derive(Subcommand, Debug)]
pub enum DebugCommand {
    /// Dump parsed commits in the given range as JSON.
    Commits {
        #[command(flatten)]
        range: RangeArgs,

        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
    },
    /// Print the detected merge style for the given range.
    MergeStyle {
        #[command(flatten)]
        range: RangeArgs,

        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
    },
    /// Print the resolved configuration as JSON.
    Config {
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
    },
    /// Run the deterministic Tier 0 classifier and emit the JSON result.
    Classify {
        #[command(flatten)]
        range: RangeArgs,

        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
    },
    /// Dump the LLM prompts that would be constructed for the range,
    /// without actually calling any LLM. Useful for tweaking voices,
    /// inspecting commit-data flow, and debugging prompt regressions.
    Prompts {
        #[command(flatten)]
        range: RangeArgs,

        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,

        /// Voice profile name (`terse`, `prose`, `simple`) or path to a custom
        /// voice markdown file (overrides TOML/default).
        #[arg(long, value_name = "NAME_OR_FILE")]
        voice: Option<String>,

        /// Inline system-prompt addendum.
        #[arg(long, value_name = "TEXT")]
        prompt: Option<String>,

        /// Include commit bodies and PR bodies in the prose-pass user
        /// prompt. Mirrors the `--rich-context` flag on the main
        /// command so the dumped prompts match what `generate` would
        /// actually send.
        #[arg(long)]
        rich_context: bool,

        /// Skip PR enrichment (offline-friendly).
        #[arg(long)]
        no_pr_enrichment: bool,

        #[command(flatten)]
        render: Box<RenderArgs>,
    },
}

/// Range-selection flags, shared by all commands that operate on a commit range.
#[derive(Args, Debug, Clone, Default)]
pub struct RangeArgs {
    /// Lower bound (exclusive). Combined with `--to`, gives `<from>..<to>`.
    #[arg(long, value_name = "REF", conflicts_with = "since_last_tag")]
    pub from: Option<String>,

    /// Upper bound. Defaults to `HEAD`.
    #[arg(long, value_name = "REF", conflicts_with = "since_last_tag")]
    pub to: Option<String>,

    /// Use the latest semver tag as `from` and `HEAD` as `to`, even when HEAD is itself tagged.
    #[arg(long)]
    pub since_last_tag: bool,
}

impl RangeArgs {
    pub fn to_spec(&self) -> crate::git::RangeSpec {
        if self.since_last_tag {
            crate::git::RangeSpec::SinceLastTag
        } else if self.from.is_some() || self.to.is_some() {
            crate::git::RangeSpec::Explicit {
                from: self.from.clone(),
                to: self.to.clone().unwrap_or_else(|| "HEAD".to_string()),
            }
        } else {
            crate::git::RangeSpec::Auto
        }
    }
}
