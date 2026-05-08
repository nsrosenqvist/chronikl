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
    DEFAULT_MAX_DIFF_TOKENS, ENV_API_KEY, ENV_BASE_URL, ENV_LICENSE_KEY, ENV_MODEL, ENV_PROVIDER,
    ENV_TELEMETRY,
};
use crate::env::Env;

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
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub license: LicenseConfig,
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryConfig {
    pub enabled: bool,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LicenseConfig {
    pub key: Option<String>,
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

        config.apply_env(env)?;
        Ok(config)
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

        // API key: chronikl-specific first, then provider-specific fallbacks.
        if let Ok(v) = env.var(ENV_API_KEY) {
            self.provider.api_key = Some(v);
        } else if let Some(provider_name) = &self.provider.name {
            if let Some(key) = lookup_provider_key(env, provider_name) {
                self.provider.api_key = Some(key);
            }
        }

        if let Ok(v) = env.var(ENV_LICENSE_KEY) {
            self.license.key = Some(v);
        }

        if let Ok(v) = env.var(ENV_TELEMETRY) {
            self.telemetry.enabled = parse_bool(&v).map_err(|m| ConfigError::InvalidValue {
                key: ENV_TELEMETRY.into(),
                message: m,
            })?;
        }

        Ok(())
    }
}

/// Map a provider name to its conventional API-key env var.
fn lookup_provider_key(env: &Env, provider: &str) -> Option<String> {
    let candidates: &[&str] = match provider.to_ascii_lowercase().as_str() {
        "anthropic" => &["ANTHROPIC_API_KEY"],
        "openai" => &["OPENAI_API_KEY"],
        "azure" => &["AZURE_OPENAI_API_KEY", "AZURE_API_KEY"],
        "gemini" => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
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

fn parse_bool(s: &str) -> Result<bool, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(format!("expected boolean (true/false/1/0), got `{other}`")),
    }
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
    if next.telemetry.enabled != TelemetryConfig::default().enabled {
        base.telemetry.enabled = next.telemetry.enabled;
    }
    if next.license.key.is_some() {
        base.license.key = next.license.key;
    }
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
        assert!(cfg.telemetry.enabled);
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
        let env = Env::mock([
            (ENV_PROVIDER, "openai"),
            (ENV_MODEL, "gpt-5-mini"),
            (ENV_TELEMETRY, "false"),
        ]);
        let cfg = Config::load(&env, dir.path(), None).unwrap();
        assert_eq!(cfg.provider.name.as_deref(), Some("openai"));
        assert_eq!(cfg.provider.model.as_deref(), Some("gpt-5-mini"));
        assert!(!cfg.telemetry.enabled);
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
    fn invalid_telemetry_value_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::mock([(ENV_TELEMETRY, "maybe")]);
        let err = Config::load(&env, dir.path(), None).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidValue { .. }));
    }

    #[test]
    fn malformed_toml_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write_toml(dir.path(), CONFIG_FILENAME, "this isn't = valid = toml");
        let env = Env::mock(Vec::<(&str, &str)>::new());
        let err = Config::load(&env, dir.path(), None).unwrap_err();
        assert!(matches!(err, ConfigError::Toml { .. }));
    }

    #[test]
    fn telemetry_accepts_truthy_strings() {
        for v in ["1", "true", "yes", "on", "TRUE", "True"] {
            let dir = tempfile::tempdir().unwrap();
            let env = Env::mock([(ENV_TELEMETRY, v)]);
            let cfg = Config::load(&env, dir.path(), None).unwrap();
            assert!(cfg.telemetry.enabled, "value `{v}` should parse to true");
        }
    }

    #[test]
    fn telemetry_accepts_falsy_strings() {
        for v in ["0", "false", "no", "off", "FALSE"] {
            let dir = tempfile::tempdir().unwrap();
            let env = Env::mock([(ENV_TELEMETRY, v)]);
            let cfg = Config::load(&env, dir.path(), None).unwrap();
            assert!(!cfg.telemetry.enabled, "value `{v}` should parse to false");
        }
    }
}
