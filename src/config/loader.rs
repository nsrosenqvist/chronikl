//! TOML + env layered configuration.
//!
//! Precedence (highest wins):
//! 1. CLI flags (merged in `main.rs` after [`Config::load`])
//! 2. Environment variables (`CHRONIKL_*` prefix; provider-specific
//!    fallbacks like `ANTHROPIC_API_KEY` for the API key)
//! 3. Repo-local `.chronikl.toml`
//! 4. User-global `~/.config/chronikl/config.toml`
//! 5. Built-in defaults

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::constants::{
    CONFIG_DIR, CONFIG_FILENAME, DEFAULT_BATCH_SIZE, DEFAULT_CONFIDENCE_THRESHOLD,
    DEFAULT_MAX_DIFF_TOKENS, ENV_API_KEY, ENV_BASE_URL, ENV_EXCLUDE_SECTIONS, ENV_FALLBACK_FIXES,
    ENV_FALLBACK_LINE, ENV_FALLBACK_OTHER, ENV_FALLBACK_PERFORMANCE, ENV_FOOTER, ENV_FORMAT,
    ENV_HEADINGS, ENV_INCLUDE_SECTIONS, ENV_MAX_BULLETS, ENV_MAX_CHARS, ENV_MODEL, ENV_PROVIDER,
    ENV_STRICT, ENV_TEMPERATURE, ENV_USER_FACING,
};
use crate::env::Env;
use crate::models::{FallbackLines, RenderSettings, Section, Temperature, TextFormat};

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not read config file {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid TOML in {path}: {source}")]
    Toml {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid value for {key}: {message}")]
    InvalidValue { key: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub voice: VoiceConfig,
    #[serde(default)]
    pub provider: ProviderConfig,
    #[serde(default)]
    pub ladder: LadderConfig,
    #[serde(default)]
    pub output: OutputConfig,
    #[serde(default)]
    pub project: ProjectConfig,
    #[serde(default)]
    pub render: RenderConfig,
}

/// `[render]` — what the notes look like and who they're for. The same
/// keys can sit in a voice file's frontmatter. Every field is optional
/// so layers merge field by field: CLI flags > env > repo TOML > global
/// TOML > the voice's frontmatter > built-in defaults. Resolve with
/// [`Config::render_settings`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderConfig {
    /// `false` renders one flat bullet list without section headings.
    pub headings: Option<bool>,
    /// Section ids to keep (e.g. `["features", "fixes"]`). Unset keeps
    /// all of them.
    pub include_sections: Option<Vec<String>>,
    /// Section ids to drop before the prose pass (e.g. `["tests"]`).
    pub exclude_sections: Option<Vec<String>>,
    /// Keep at most this many bullets.
    pub max_bullets: Option<usize>,
    /// Keep the output within this many characters, cut at whole
    /// bullets.
    pub max_chars: Option<usize>,
    /// `false` drops the "Full Changelog" compare link.
    pub footer: Option<bool>,
    /// `markdown` (default) or `plain`.
    pub format: Option<TextFormat>,
    /// Notes for the product's users: keep only what they'd notice, and
    /// write a fallback line when nothing is user-facing.
    pub user_facing: Option<bool>,
    /// "Nothing user-facing" line for every kind of range.
    pub fallback_line: Option<String>,
    /// … for a range with bug fixes.
    pub fallback_fixes: Option<String>,
    /// … for a range with performance work (and no fixes).
    pub fallback_performance: Option<String>,
    /// … for any other range.
    pub fallback_other: Option<String>,
}

impl RenderConfig {
    /// Overlay `higher` on `self`: every field `higher` sets wins.
    pub fn overlay(&mut self, higher: RenderConfig) {
        macro_rules! take {
            ($($f:ident),*) => {$(
                if higher.$f.is_some() {
                    self.$f = higher.$f;
                }
            )*};
        }
        take!(
            headings,
            include_sections,
            exclude_sections,
            max_bullets,
            max_chars,
            footer,
            format,
            user_facing,
            fallback_line,
            fallback_fixes,
            fallback_performance,
            fallback_other
        );
    }
}
/// Project-level metadata folded into the prose-pass user prompt so the
/// model has a "what this project is" signal. Defaults to auto-detection
/// from the repo's language manifest and `README.md`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// Override the project description. When set, takes precedence over
    /// any `description` field in `Cargo.toml` / `package.json` /
    /// `pyproject.toml`.
    pub description: Option<String>,
    /// Override the README path. When set, this file is read instead of
    /// auto-detecting `README.md` / `README.rst` in the repo root.
    pub readme: Option<PathBuf>,
    /// Disable README auto-detection entirely. Useful when the README
    /// contains content that shouldn't be sent to an LLM.
    #[serde(default)]
    pub no_readme: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VoiceConfig {
    /// Path to a custom voice markdown file. Wins over `profile` when
    /// both are set in the same TOML.
    pub path: Option<PathBuf>,
    /// Bundled voice profile name (`terse`, `prose`, or the alias
    /// `default`). Used when `path` is unset.
    pub profile: Option<String>,
    /// Appended to the resolved system prompt at use-site.
    pub extra_instructions: Option<String>,
    /// Include commit bodies and PR bodies in the prose-pass user
    /// prompt. Off by default — opt-in for richer voices.
    #[serde(default)]
    pub rich_context: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// e.g. `"anthropic"`, `"openai"`, `"gemini"`, `"openai-compatible"`.
    pub name: Option<String>,
    pub model: Option<String>,
    /// Provider API key. Sourced from env in production; this field exists
    /// so tests/mocks can supply one programmatically.
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    /// Sampling temperature: a number, or `"none"` to send none. Unset
    /// sends [`crate::constants::DEFAULT_TEMPERATURE`].
    pub temperature: Option<Temperature>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LadderConfig {
    pub agent_fallback: bool,
    pub max_diff_tokens: usize,
    pub confidence_threshold: f32,
    pub batch_size: usize,
}

impl Default for LadderConfig {
    fn default() -> Self {
        Self {
            agent_fallback: false,
            max_diff_tokens: DEFAULT_MAX_DIFF_TOKENS,
            confidence_threshold: DEFAULT_CONFIDENCE_THRESHOLD,
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Markdown,
    Json,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OutputConfig {
    #[serde(default)]
    pub format: OutputFormat,
    pub path: Option<PathBuf>,
    /// Fail the run (non-zero exit, nothing written) when the notes
    /// would fall back to the deterministic renderer because an LLM
    /// pass failed or no provider was usable.
    pub strict: Option<bool>,
}

impl Config {
    /// Load configuration with full precedence:
    /// defaults → global TOML → repo TOML → environment variables.
    ///
    /// `repo_root` is the directory to search for `.chronikl.toml`.
    /// `home` is the directory to search for `<CONFIG_DIR>/config.toml`
    /// (typically the value of `dirs::config_dir()`); pass `None` to skip
    /// global config (useful in tests).
    pub fn load(env: &Env, repo_root: &Path, home: Option<&Path>) -> Result<Self, ConfigError> {
        let mut config = Self::default();

        if let Some(home) = home {
            let global_path = home.join(CONFIG_DIR).join("config.toml");
            if global_path.exists() {
                let next = read_toml(&global_path)?;
                config = merge(config, next);
            }
        }

        let repo_path = repo_root.join(CONFIG_FILENAME);
        if repo_path.exists() {
            let next = read_toml(&repo_path)?;
            config = merge(config, next);
        }

        config.anchor_paths(repo_root);
        config.apply_env(env)?;
        Ok(config)
    }

    /// Resolve relative file paths from TOML (`voice.path`,
    /// `project.readme`) against the repo root, so a repo's
    /// `.chronikl.toml` works whatever directory chronikl runs from
    /// (e.g. with `--repo`). Paths from CLI flags stay relative to the
    /// working directory.
    fn anchor_paths(&mut self, repo_root: &Path) {
        for path in [&mut self.voice.path, &mut self.project.readme]
            .into_iter()
            .flatten()
        {
            if path.is_relative() {
                *path = repo_root.join(&*path);
            }
        }
    }

    fn apply_env(&mut self, env: &Env) -> Result<(), ConfigError> {
        if let Ok(v) = env.var(ENV_PROVIDER) {
            self.provider.name = Some(v);
        }
        if let Ok(v) = env.var(ENV_MODEL) {
            self.provider.model = Some(v);
        }
        if let Ok(v) = env.var(ENV_BASE_URL) {
            self.provider.base_url = Some(v);
        }
        if let Some(v) = non_empty_var(env, ENV_TEMPERATURE) {
            self.provider.temperature = Some(v.parse().map_err(
                |e: crate::models::ParseTemperatureError| invalid(ENV_TEMPERATURE, e.to_string()),
            )?);
        }
        if let Some(v) = non_empty_var(env, ENV_STRICT) {
            self.output.strict = Some(parse_bool(ENV_STRICT, &v)?);
        }
        if let Some(v) = non_empty_var(env, ENV_USER_FACING) {
            self.render.user_facing = Some(parse_bool(ENV_USER_FACING, &v)?);
        }
        for (key, field) in [
            (ENV_FALLBACK_LINE, &mut self.render.fallback_line),
            (ENV_FALLBACK_FIXES, &mut self.render.fallback_fixes),
            (
                ENV_FALLBACK_PERFORMANCE,
                &mut self.render.fallback_performance,
            ),
            (ENV_FALLBACK_OTHER, &mut self.render.fallback_other),
        ] {
            if let Some(v) = non_empty_var(env, key) {
                *field = Some(v);
            }
        }
        if let Some(v) = non_empty_var(env, ENV_HEADINGS) {
            self.render.headings = Some(parse_bool(ENV_HEADINGS, &v)?);
        }
        if let Ok(v) = env.var(ENV_INCLUDE_SECTIONS) {
            self.render.include_sections = Some(split_list(&v));
        }
        if let Ok(v) = env.var(ENV_EXCLUDE_SECTIONS) {
            self.render.exclude_sections = Some(split_list(&v));
        }
        if let Some(v) = non_empty_var(env, ENV_MAX_BULLETS) {
            self.render.max_bullets = Some(parse_count(ENV_MAX_BULLETS, &v)?);
        }
        if let Some(v) = non_empty_var(env, ENV_MAX_CHARS) {
            self.render.max_chars = Some(parse_count(ENV_MAX_CHARS, &v)?);
        }
        if let Some(v) = non_empty_var(env, ENV_FOOTER) {
            self.render.footer = Some(parse_bool(ENV_FOOTER, &v)?);
        }
        if let Some(v) = non_empty_var(env, ENV_FORMAT) {
            self.render.format = Some(v.parse().map_err(|_| {
                invalid(
                    ENV_FORMAT,
                    format!("expected `markdown` or `plain`, got `{v}`"),
                )
            })?);
        }

        // API key: chronikl-specific first, then provider-specific fallbacks.
        if let Ok(v) = env.var(ENV_API_KEY) {
            self.provider.api_key = Some(v);
        } else if let Some(provider_name) = &self.provider.name {
            if let Some(key) = lookup_provider_key(env, provider_name) {
                self.provider.api_key = Some(key);
            }
        }

        Ok(())
    }
}

impl Config {
    /// Whether `--strict` behavior is on for this run.
    pub fn strict(&self) -> bool {
        self.output.strict.unwrap_or(false)
    }

    /// The temperature to send, or `None` to leave it out.
    pub fn temperature(&self) -> Option<f64> {
        self.provider
            .temperature
            .unwrap_or(Temperature::Value(crate::constants::DEFAULT_TEMPERATURE))
            .as_option()
    }

    /// Resolve the render settings: `[render]` from flags, env and TOML
    /// over `voice`'s frontmatter, over the built-in defaults.
    ///
    /// One rule crosses fields: an `include_sections` list set above the
    /// voice says exactly what to keep, so the voice's
    /// `exclude_sections` don't apply unless `exclude_sections` is set
    /// at that level too.
    pub fn render_settings(&self, voice: &RenderConfig) -> Result<RenderSettings, ConfigError> {
        let mut r = voice.clone();
        if self.render.include_sections.is_some() && self.render.exclude_sections.is_none() {
            r.exclude_sections = None;
        }
        r.overlay(self.render.clone());
        resolve_render(&r)
    }
}

/// Turn a merged [`RenderConfig`] into [`RenderSettings`].
pub fn resolve_render(r: &RenderConfig) -> Result<RenderSettings, ConfigError> {
    let defaults = RenderSettings::default();
    let include_sections = match &r.include_sections {
        Some(list) if list.iter().any(|s| !s.trim().is_empty()) => {
            Some(parse_sections("render.include_sections", list)?)
        }
        _ => None,
    };
    let exclude_sections = match &r.exclude_sections {
        Some(list) => parse_sections("render.exclude_sections", list)?,
        None => Vec::new(),
    };
    for (key, value) in [
        ("render.max_bullets", r.max_bullets),
        ("render.max_chars", r.max_chars),
    ] {
        if value == Some(0) {
            return Err(invalid(key, "must be at least 1".to_string()));
        }
    }
    Ok(RenderSettings {
        headings: r.headings.unwrap_or(defaults.headings),
        include_sections,
        exclude_sections,
        max_bullets: r.max_bullets,
        max_chars: r.max_chars,
        footer: r.footer.unwrap_or(defaults.footer),
        format: r.format.unwrap_or(defaults.format),
        user_facing: r.user_facing.unwrap_or(defaults.user_facing),
        fallback_lines: FallbackLines {
            line: r.fallback_line.clone(),
            fixes: r.fallback_fixes.clone(),
            performance: r.fallback_performance.clone(),
            other: r.fallback_other.clone(),
        },
    })
}

fn invalid(key: &str, message: String) -> ConfigError {
    ConfigError::InvalidValue {
        key: key.to_string(),
        message,
    }
}

fn non_empty_var(env: &Env, key: &str) -> Option<String> {
    env.var(key).ok().filter(|v| !v.trim().is_empty())
}

fn parse_bool(key: &str, raw: &str) -> Result<bool, ConfigError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(invalid(key, format!("expected true or false, got `{raw}`"))),
    }
}

fn parse_count(key: &str, raw: &str) -> Result<usize, ConfigError> {
    raw.trim()
        .parse::<usize>()
        .map_err(|_| invalid(key, format!("expected a whole number, got `{raw}`")))
}

fn split_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parse section ids leniently (`fixes`, `bug-fixes`, `docs`, …).
fn parse_sections(key: &str, raw: &[String]) -> Result<Vec<Section>, ConfigError> {
    raw.iter()
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            Section::parse_lenient(s).ok_or_else(|| {
                invalid(
                    key,
                    format!(
                        "unknown section `{s}` (known: breaking, features, fixes, performance, \
                         refactor, docs, tests, build, ci, chore, other)"
                    ),
                )
            })
        })
        .collect()
}

/// Map a provider name to its conventional API-key env var.
fn lookup_provider_key(env: &Env, provider: &str) -> Option<String> {
    let candidates: &[&str] = match provider.to_ascii_lowercase().as_str() {
        "anthropic" => &["ANTHROPIC_API_KEY"],
        "openai" => &["OPENAI_API_KEY"],
        "azure" => &["AZURE_OPENAI_API_KEY", "AZURE_API_KEY"],
        "gemini" => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        // GitHub Models accepts the standard CI-injected GITHUB_TOKEN as
        // an API key, so workflows with `permissions: models: read` need
        // no extra secret. GITHUB_MODELS_TOKEN is checked first so users
        // can keep a dedicated fine-grained PAT separate from a general
        // GITHUB_TOKEN.
        "github-models" => &["GITHUB_MODELS_TOKEN", "GITHUB_TOKEN", "GH_TOKEN"],
        "groq" => &["GROQ_API_KEY"],
        "mistral" => &["MISTRAL_API_KEY"],
        "deepseek" => &["DEEPSEEK_API_KEY"],
        "xai" => &["XAI_API_KEY"],
        "cohere" => &["COHERE_API_KEY"],
        "openrouter" => &["OPENROUTER_API_KEY"],
        "perplexity" => &["PERPLEXITY_API_KEY"],
        "together" => &["TOGETHER_API_KEY"],
        "moonshot" => &["MOONSHOT_API_KEY"],
        "huggingface" => &["HF_API_KEY", "HUGGINGFACE_API_KEY"],
        "ollama" => &[], // local — no key needed
        _ => &[],
    };
    candidates.iter().find_map(|k| env.var(k).ok())
}

fn read_toml(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    toml::from_str::<Config>(&text).map_err(|e| ConfigError::Toml {
        path: path.display().to_string(),
        source: e,
    })
}

/// Layer two configs: every `Some` field in `next` overrides the
/// corresponding field in `base`. Lists/maps are not deeply merged —
/// chronikl's config is small enough that whole-section override is fine.
fn merge(mut base: Config, next: Config) -> Config {
    if next.voice.path.is_some() {
        base.voice.path = next.voice.path;
    }
    if next.voice.profile.is_some() {
        base.voice.profile = next.voice.profile;
    }
    if next.voice.extra_instructions.is_some() {
        base.voice.extra_instructions = next.voice.extra_instructions;
    }
    if next.voice.rich_context != VoiceConfig::default().rich_context {
        base.voice.rich_context = next.voice.rich_context;
    }
    if next.provider.name.is_some() {
        base.provider.name = next.provider.name;
    }
    if next.provider.model.is_some() {
        base.provider.model = next.provider.model;
    }
    if next.provider.api_key.is_some() {
        base.provider.api_key = next.provider.api_key;
    }
    if next.provider.base_url.is_some() {
        base.provider.base_url = next.provider.base_url;
    }
    if next.provider.temperature.is_some() {
        base.provider.temperature = next.provider.temperature;
    }
    // Ladder: TOML provides full struct via #[serde(default)], so we treat
    // any deserialized non-default values as overrides. Simpler to compare
    // each field against its default than to thread Option<...> through.
    let default_ladder = LadderConfig::default();
    if next.ladder != default_ladder {
        base.ladder = next.ladder;
    }
    if next.output.format != OutputFormat::default() {
        base.output.format = next.output.format;
    }
    if next.output.path.is_some() {
        base.output.path = next.output.path;
    }
    if next.output.strict.is_some() {
        base.output.strict = next.output.strict;
    }
    if next.project.description.is_some() {
        base.project.description = next.project.description;
    }
    if next.project.readme.is_some() {
        base.project.readme = next.project.readme;
    }
    if next.project.no_readme != ProjectConfig::default().no_readme {
        base.project.no_readme = next.project.no_readme;
    }
    base.render.overlay(next.render);
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_toml(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn defaults_are_sensible() {
        let cfg = Config::default();
        assert_eq!(cfg.ladder.batch_size, DEFAULT_BATCH_SIZE);
        assert_eq!(cfg.ladder.max_diff_tokens, DEFAULT_MAX_DIFF_TOKENS);
        assert!(matches!(cfg.output.format, OutputFormat::Markdown));
    }

    #[test]
    fn loads_repo_toml() {
        let dir = tempfile::tempdir().unwrap();
        write_toml(
            dir.path(),
            CONFIG_FILENAME,
            r#"
[provider]
name = "anthropic"
model = "claude-sonnet-4-6"

[ladder]
agent_fallback = true
max_diff_tokens = 8000
confidence_threshold = 0.5
batch_size = 25
"#,
        );
        let env = Env::mock(Vec::<(&str, &str)>::new());
        let cfg = Config::load(&env, dir.path(), None).unwrap();
        assert_eq!(cfg.provider.name.as_deref(), Some("anthropic"));
        assert_eq!(cfg.provider.model.as_deref(), Some("claude-sonnet-4-6"));
        assert!(cfg.ladder.agent_fallback);
        assert_eq!(cfg.ladder.max_diff_tokens, 8000);
        assert_eq!(cfg.ladder.batch_size, 25);
    }

    #[test]
    fn repo_toml_overrides_global_toml() {
        let global_root = tempfile::tempdir().unwrap();
        let global_dir = global_root.path().join(CONFIG_DIR);
        std::fs::create_dir_all(&global_dir).unwrap();
        write_toml(
            &global_dir,
            "config.toml",
            r#"[provider]
name = "openai"
model = "gpt-5"
"#,
        );

        let repo_dir = tempfile::tempdir().unwrap();
        write_toml(
            repo_dir.path(),
            CONFIG_FILENAME,
            r#"[provider]
name = "anthropic"
"#,
        );

        let env = Env::mock(Vec::<(&str, &str)>::new());
        let cfg = Config::load(&env, repo_dir.path(), Some(global_root.path())).unwrap();
        // Repo override wins on `name`, but global `model` survives.
        assert_eq!(cfg.provider.name.as_deref(), Some("anthropic"));
        assert_eq!(cfg.provider.model.as_deref(), Some("gpt-5"));
    }

    #[test]
    fn env_overrides_toml() {
        let dir = tempfile::tempdir().unwrap();
        write_toml(
            dir.path(),
            CONFIG_FILENAME,
            r#"[provider]
name = "anthropic"
model = "claude-sonnet-4-6"
"#,
        );
        let env = Env::mock([(ENV_PROVIDER, "openai"), (ENV_MODEL, "gpt-5-mini")]);
        let cfg = Config::load(&env, dir.path(), None).unwrap();
        assert_eq!(cfg.provider.name.as_deref(), Some("openai"));
        assert_eq!(cfg.provider.model.as_deref(), Some("gpt-5-mini"));
    }

    #[test]
    fn provider_specific_api_key_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::mock([
            (ENV_PROVIDER, "anthropic"),
            ("ANTHROPIC_API_KEY", "sk-ant-test"),
        ]);
        let cfg = Config::load(&env, dir.path(), None).unwrap();
        assert_eq!(cfg.provider.api_key.as_deref(), Some("sk-ant-test"));
    }

    #[test]
    fn github_models_resolves_github_token() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::mock([
            (ENV_PROVIDER, "github-models"),
            ("GITHUB_TOKEN", "ghp_ci_test"),
        ]);
        let cfg = Config::load(&env, dir.path(), None).unwrap();
        assert_eq!(cfg.provider.api_key.as_deref(), Some("ghp_ci_test"));
    }

    #[test]
    fn github_models_token_beats_generic_github_token() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::mock([
            (ENV_PROVIDER, "github-models"),
            ("GITHUB_MODELS_TOKEN", "dedicated-pat"),
            ("GITHUB_TOKEN", "generic-ci-token"),
        ]);
        let cfg = Config::load(&env, dir.path(), None).unwrap();
        assert_eq!(cfg.provider.api_key.as_deref(), Some("dedicated-pat"));
    }

    #[test]
    fn chronikl_api_key_takes_precedence_over_provider_key() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::mock([
            (ENV_PROVIDER, "anthropic"),
            (ENV_API_KEY, "from-chronikl"),
            ("ANTHROPIC_API_KEY", "from-anthropic"),
        ]);
        let cfg = Config::load(&env, dir.path(), None).unwrap();
        assert_eq!(cfg.provider.api_key.as_deref(), Some("from-chronikl"));
    }

    #[test]
    fn malformed_toml_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write_toml(dir.path(), CONFIG_FILENAME, "this isn't = valid = toml");
        let env = Env::mock(Vec::<(&str, &str)>::new());
        let err = Config::load(&env, dir.path(), None).unwrap_err();
        assert!(matches!(err, ConfigError::Toml { .. }));
    }

    fn simple_like_voice() -> RenderConfig {
        RenderConfig {
            headings: Some(false),
            footer: Some(false),
            exclude_sections: Some(vec!["docs".into(), "tests".into(), "ci".into()]),
            user_facing: Some(true),
            max_bullets: Some(5),
            ..RenderConfig::default()
        }
    }

    #[test]
    fn render_defaults_without_voice_settings() {
        let r = Config::default()
            .render_settings(&RenderConfig::default())
            .unwrap();
        assert_eq!(r, RenderSettings::default());
    }

    #[test]
    fn voice_frontmatter_supplies_defaults() {
        let r = Config::default()
            .render_settings(&simple_like_voice())
            .unwrap();
        assert!(!r.headings);
        assert!(!r.footer);
        assert!(r.user_facing);
        assert_eq!(r.max_bullets, Some(5));
        assert!(r.keeps(Section::Features));
        assert!(!r.keeps(Section::Tests));
    }

    #[test]
    fn config_beats_voice_frontmatter() {
        let mut cfg = Config::default();
        cfg.render.headings = Some(true);
        cfg.render.footer = Some(true);
        cfg.render.exclude_sections = Some(vec!["tests".into()]);
        cfg.render.max_bullets = Some(3);
        let r = cfg.render_settings(&simple_like_voice()).unwrap();
        assert!(r.headings);
        assert!(r.footer);
        assert_eq!(r.exclude_sections, vec![Section::Tests]);
        assert!(r.keeps(Section::Documentation));
        assert_eq!(r.max_bullets, Some(3));
        assert!(r.user_facing);
    }

    #[test]
    fn include_list_above_the_voice_replaces_its_exclusions() {
        let mut cfg = Config::default();
        cfg.render.include_sections = Some(vec!["features".into(), "docs".into()]);
        let r = cfg.render_settings(&simple_like_voice()).unwrap();
        assert!(r.exclude_sections.is_empty());
        assert!(r.keeps(Section::Documentation));
        assert!(!r.keeps(Section::BugFixes));
    }

    #[test]
    fn empty_exclude_list_clears_the_voice_exclusions() {
        let mut cfg = Config::default();
        cfg.render.exclude_sections = Some(vec![String::new()]);
        let r = cfg.render_settings(&simple_like_voice()).unwrap();
        assert!(r.exclude_sections.is_empty());
    }

    #[test]
    fn section_ids_parse_leniently_and_reject_unknowns() {
        let mut cfg = Config::default();
        cfg.render.exclude_sections = Some(vec!["fixes".into(), "bug-fixes".into(), "perf".into()]);
        let r = cfg.render_settings(&RenderConfig::default()).unwrap();
        assert_eq!(
            r.exclude_sections,
            vec![Section::BugFixes, Section::BugFixes, Section::Performance]
        );
        cfg.render.exclude_sections = Some(vec!["screenshots".into()]);
        assert!(matches!(
            cfg.render_settings(&RenderConfig::default()),
            Err(ConfigError::InvalidValue { .. })
        ));
    }

    #[test]
    fn zero_limits_are_rejected() {
        let mut cfg = Config::default();
        cfg.render.max_chars = Some(0);
        assert!(cfg.render_settings(&RenderConfig::default()).is_err());
    }

    #[test]
    fn unknown_render_keys_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        write_toml(dir.path(), CONFIG_FILENAME, "[render]\nheading = false\n");
        let env = Env::mock(Vec::<(&str, &str)>::new());
        assert!(matches!(
            Config::load(&env, dir.path(), None),
            Err(ConfigError::Toml { .. })
        ));
    }

    #[test]
    fn render_toml_layers_field_by_field() {
        let global_root = tempfile::tempdir().unwrap();
        let global_dir = global_root.path().join(CONFIG_DIR);
        std::fs::create_dir_all(&global_dir).unwrap();
        write_toml(
            &global_dir,
            "config.toml",
            "[render]\nuser_facing = true\nmax_chars = 500\n",
        );
        let repo_dir = tempfile::tempdir().unwrap();
        write_toml(
            repo_dir.path(),
            CONFIG_FILENAME,
            "[render]\nformat = \"plain\"\nmax_chars = 450\nfallback_fixes = \"Fixes.\"\n",
        );
        let env = Env::mock(Vec::<(&str, &str)>::new());
        let cfg = Config::load(&env, repo_dir.path(), Some(global_root.path())).unwrap();
        let r = cfg.render_settings(&RenderConfig::default()).unwrap();
        assert!(r.user_facing);
        assert_eq!(r.format, TextFormat::Plain);
        assert_eq!(r.max_chars, Some(450));
        assert_eq!(r.fallback_lines.fixes.as_deref(), Some("Fixes."));
    }

    #[test]
    fn render_env_vars_override_toml() {
        let dir = tempfile::tempdir().unwrap();
        write_toml(
            dir.path(),
            CONFIG_FILENAME,
            "[render]\nheadings = true\nmax_bullets = 9\n",
        );
        let env = Env::mock([
            (ENV_USER_FACING, "true"),
            (ENV_HEADINGS, "false"),
            (ENV_MAX_BULLETS, "5"),
            (ENV_MAX_CHARS, "450"),
            (ENV_FOOTER, "off"),
            (ENV_FORMAT, "plain"),
            (ENV_INCLUDE_SECTIONS, "features, fixes"),
            (ENV_EXCLUDE_SECTIONS, ""),
            (ENV_FALLBACK_LINE, "Updates."),
            (ENV_FALLBACK_PERFORMANCE, "Faster."),
            (ENV_STRICT, "1"),
            (ENV_TEMPERATURE, "none"),
        ]);
        let cfg = Config::load(&env, dir.path(), None).unwrap();
        let r = cfg.render_settings(&simple_like_voice()).unwrap();
        assert!(r.user_facing);
        assert!(!r.headings);
        assert!(!r.footer);
        assert_eq!(r.max_bullets, Some(5));
        assert_eq!(r.max_chars, Some(450));
        assert_eq!(r.format, TextFormat::Plain);
        assert_eq!(
            r.include_sections,
            Some(vec![Section::Features, Section::BugFixes])
        );
        assert!(r.exclude_sections.is_empty());
        assert_eq!(r.fallback_lines.line.as_deref(), Some("Updates."));
        assert_eq!(r.fallback_lines.performance.as_deref(), Some("Faster."));
        assert!(cfg.strict());
        assert_eq!(cfg.temperature(), None);
    }

    #[test]
    fn invalid_env_values_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        for (k, v) in [
            (ENV_HEADINGS, "maybe"),
            (ENV_MAX_CHARS, "lots"),
            (ENV_USER_FACING, "maybe"),
            (ENV_FORMAT, "html"),
            (ENV_TEMPERATURE, "hot"),
        ] {
            let env = Env::mock([(k, v)]);
            let err = Config::load(&env, dir.path(), None).unwrap_err();
            assert!(matches!(err, ConfigError::InvalidValue { .. }), "{k}={v}");
        }
    }

    #[test]
    fn temperature_defaults_to_zero_and_reads_toml() {
        assert_eq!(Config::default().temperature(), Some(0.0));
        let dir = tempfile::tempdir().unwrap();
        write_toml(
            dir.path(),
            CONFIG_FILENAME,
            "[provider]\ntemperature = \"none\"\n",
        );
        let env = Env::mock(Vec::<(&str, &str)>::new());
        let cfg = Config::load(&env, dir.path(), None).unwrap();
        assert_eq!(cfg.temperature(), None);
        assert!(!cfg.strict());
    }

    #[test]
    fn relative_toml_paths_resolve_against_the_repo_root() {
        let dir = tempfile::tempdir().unwrap();
        write_toml(
            dir.path(),
            CONFIG_FILENAME,
            "[voice]\npath = \"tools/voice.md\"\n[project]\nreadme = \"/abs/README.md\"\n",
        );
        let env = Env::mock(Vec::<(&str, &str)>::new());
        let cfg = Config::load(&env, dir.path(), None).unwrap();
        assert_eq!(cfg.voice.path, Some(dir.path().join("tools/voice.md")));
        assert_eq!(cfg.project.readme, Some(PathBuf::from("/abs/README.md")));
    }

    #[test]
    fn temperature_defaults_to_zero_and_layers() {
        assert_eq!(Config::default().temperature(), Some(0.0));
        let dir = tempfile::tempdir().unwrap();
        write_toml(
            dir.path(),
            CONFIG_FILENAME,
            "[provider]\ntemperature = 0.3\n",
        );
        let env = Env::mock(Vec::<(&str, &str)>::new());
        let cfg = Config::load(&env, dir.path(), None).unwrap();
        assert_eq!(cfg.temperature(), Some(0.3));
        let env = Env::mock([(ENV_TEMPERATURE, "none")]);
        let cfg = Config::load(&env, dir.path(), None).unwrap();
        assert_eq!(cfg.temperature(), None);
        let env = Env::mock([(ENV_TEMPERATURE, "hot")]);
        assert!(matches!(
            Config::load(&env, dir.path(), None),
            Err(ConfigError::InvalidValue { .. })
        ));
    }
}
