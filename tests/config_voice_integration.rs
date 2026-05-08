//! Integration tests for the config and voice layers.
//!
//! Exercises the full load path: real TOML files on disk, mock env vars,
//! and the bundled default voice. Complements the inline unit tests by
//! covering the I/O seams (filesystem reads, layered overrides) end-to-end.

use std::path::Path;

use chronikl::config::{Config, OutputFormat};
use chronikl::constants::{
    CONFIG_DIR, CONFIG_FILENAME, ENV_API_KEY, ENV_MODEL, ENV_PROVIDER, ENV_TELEMETRY,
};
use chronikl::env::Env;
use chronikl::voice;

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

#[test]
fn full_layering_global_repo_env() {
    // Global config sets defaults the user has across all projects.
    let global_root = tempfile::tempdir().unwrap();
    let global_path = global_root.path().join(CONFIG_DIR).join("config.toml");
    write(
        &global_path,
        r#"
[provider]
name = "anthropic"
model = "claude-sonnet-4-6"

[ladder]
batch_size = 100
max_diff_tokens = 4000
agent_fallback = false
confidence_threshold = 0.6

[telemetry]
enabled = true
"#,
    );

    // Repo overrides the model and disables telemetry locally.
    let repo = tempfile::tempdir().unwrap();
    write(
        &repo.path().join(CONFIG_FILENAME),
        r#"
[provider]
model = "claude-opus-4-7"

[telemetry]
enabled = false
"#,
    );

    // Env overrides model again.
    let env = Env::mock([(ENV_MODEL, "claude-haiku-4-5"), (ENV_API_KEY, "k")]);

    let cfg = Config::load(&env, repo.path(), Some(global_root.path())).unwrap();
    assert_eq!(cfg.provider.name.as_deref(), Some("anthropic"));
    assert_eq!(cfg.provider.model.as_deref(), Some("claude-haiku-4-5"));
    assert_eq!(cfg.provider.api_key.as_deref(), Some("k"));
    assert_eq!(cfg.ladder.batch_size, 100);
    assert!(!cfg.telemetry.enabled);
}

#[test]
fn voice_path_in_toml_is_loadable() {
    let repo = tempfile::tempdir().unwrap();

    // User's repo has a release voice committed alongside the config.
    let voice_file = repo.path().join("release-voice.md");
    write(
        &voice_file,
        "Write release notes like a pirate. Use 'arr' liberally.\n",
    );

    write(
        &repo.path().join(CONFIG_FILENAME),
        &format!(
            r#"
[voice]
path = {voice_path:?}
extra_instructions = "Mention the release manager's name."
"#,
            voice_path = voice_file.to_string_lossy().into_owned()
        ),
    );

    let env = Env::mock(Vec::<(&str, &str)>::new());
    let cfg = Config::load(&env, repo.path(), None).unwrap();
    assert_eq!(cfg.voice.path.as_deref(), Some(voice_file.as_path()));
    assert!(cfg.voice.extra_instructions.is_some());

    // The path resolves to a file that voice::load_from_file can read.
    let v = voice::load_from_file(cfg.voice.path.as_deref().unwrap()).unwrap();
    assert!(v.is_custom);
    assert!(v.system_prompt.contains("pirate"));
}

#[test]
fn bundled_default_voice_is_usable_without_any_files() {
    let v = voice::default();
    assert!(!v.system_prompt.is_empty());
    assert_eq!(v.bundled_name, Some("terse"));
    // Sanity-check the default has the substantive guidance the prose
    // pass relies on.
    assert!(v.system_prompt.contains("Markdown"));
}

#[test]
fn missing_voice_file_surfaces_io_error() {
    let bogus = Path::new("/tmp/chronikl-test-no-such-file.md");
    let err = voice::load_from_file(bogus).unwrap_err();
    assert!(format!("{err:#}").contains("/tmp/chronikl-test-no-such-file.md"));
}

#[test]
fn voice_profile_in_toml_loads_bundled() {
    let repo = tempfile::tempdir().unwrap();
    write(
        &repo.path().join(CONFIG_FILENAME),
        r#"
[voice]
profile = "prose"
"#,
    );

    let env = Env::mock(Vec::<(&str, &str)>::new());
    let cfg = Config::load(&env, repo.path(), None).unwrap();
    assert_eq!(cfg.voice.profile.as_deref(), Some("prose"));

    let v = voice::resolve(
        None,
        cfg.voice.path.as_deref(),
        cfg.voice.profile.as_deref(),
    )
    .unwrap();
    assert_eq!(v.bundled_name, Some("prose"));
    assert!(v.system_prompt.contains("Marquee bullets"));
}

#[test]
fn voice_path_beats_voice_profile_when_both_set() {
    let repo = tempfile::tempdir().unwrap();
    let voice_file = repo.path().join("custom.md");
    write(&voice_file, "Custom voice body.\n");

    write(
        &repo.path().join(CONFIG_FILENAME),
        &format!(
            r#"
[voice]
path = {p:?}
profile = "prose"
"#,
            p = voice_file.to_string_lossy().into_owned(),
        ),
    );

    let env = Env::mock(Vec::<(&str, &str)>::new());
    let cfg = Config::load(&env, repo.path(), None).unwrap();

    let v = voice::resolve(
        None,
        cfg.voice.path.as_deref(),
        cfg.voice.profile.as_deref(),
    )
    .unwrap();
    assert!(v.is_custom, "[voice].path should win over [voice].profile");
    assert_eq!(v.system_prompt, "Custom voice body.");
}

#[test]
fn unknown_voice_profile_surfaces_clear_error() {
    let err = voice::resolve(None, None, Some("operatic")).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("operatic"),
        "error should name the unknown profile: {msg}"
    );
    assert!(
        msg.contains("terse"),
        "error should list known profiles: {msg}"
    );
    assert!(
        msg.contains("prose"),
        "error should list known profiles: {msg}"
    );
}

#[test]
fn voice_rich_context_round_trips_through_toml() {
    let repo = tempfile::tempdir().unwrap();
    write(
        &repo.path().join(CONFIG_FILENAME),
        r#"
[voice]
profile = "prose"
rich_context = true
"#,
    );

    let env = Env::mock(Vec::<(&str, &str)>::new());
    let cfg = Config::load(&env, repo.path(), None).unwrap();
    assert!(cfg.voice.rich_context);
    assert_eq!(cfg.voice.profile.as_deref(), Some("prose"));
}

#[test]
fn voice_rich_context_defaults_to_false_when_unset() {
    let repo = tempfile::tempdir().unwrap();
    let env = Env::mock(Vec::<(&str, &str)>::new());
    let cfg = Config::load(&env, repo.path(), None).unwrap();
    assert!(!cfg.voice.rich_context);
}

#[test]
fn cli_voice_name_overrides_toml_path() {
    let repo = tempfile::tempdir().unwrap();
    let voice_file = repo.path().join("custom.md");
    write(&voice_file, "Custom voice body.\n");

    write(
        &repo.path().join(CONFIG_FILENAME),
        &format!(
            r#"
[voice]
path = {p:?}
"#,
            p = voice_file.to_string_lossy().into_owned(),
        ),
    );

    let env = Env::mock(Vec::<(&str, &str)>::new());
    let cfg = Config::load(&env, repo.path(), None).unwrap();

    // Mimic the resolver call from main.rs / debug_prompts when the user
    // passes --voice prose on the CLI.
    let v = voice::resolve(
        Some("prose"),
        cfg.voice.path.as_deref(),
        cfg.voice.profile.as_deref(),
    )
    .unwrap();
    assert_eq!(v.bundled_name, Some("prose"));
}

#[test]
fn output_format_round_trips_through_toml() {
    let repo = tempfile::tempdir().unwrap();
    write(
        &repo.path().join(CONFIG_FILENAME),
        r#"
[output]
format = "json"
path = "release.json"
"#,
    );
    let env = Env::mock(Vec::<(&str, &str)>::new());
    let cfg = Config::load(&env, repo.path(), None).unwrap();
    assert_eq!(cfg.output.format, OutputFormat::Json);
    assert_eq!(cfg.output.path.as_deref(), Some(Path::new("release.json")));
}

#[test]
fn provider_specific_api_key_picked_up_when_chronikl_one_is_absent() {
    let repo = tempfile::tempdir().unwrap();
    write(
        &repo.path().join(CONFIG_FILENAME),
        r#"
[provider]
name = "openai"
"#,
    );
    let env = Env::mock([(ENV_PROVIDER, "openai"), ("OPENAI_API_KEY", "sk-openai")]);
    let cfg = Config::load(&env, repo.path(), None).unwrap();
    assert_eq!(cfg.provider.api_key.as_deref(), Some("sk-openai"));
}

#[test]
fn telemetry_off_via_env() {
    let repo = tempfile::tempdir().unwrap();
    let env = Env::mock([(ENV_TELEMETRY, "false")]);
    let cfg = Config::load(&env, repo.path(), None).unwrap();
    assert!(!cfg.telemetry.enabled);
}
