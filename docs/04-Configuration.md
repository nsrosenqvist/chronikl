# Configuration

chronikl reads from three layers in order of increasing precedence:

1. Built-in defaults
2. Global config: `~/.config/chronikl/config.toml`
3. Repo-local config: `.chronikl.toml`
4. Environment variables (`CHRONIKL_*` and provider-specific keys)
5. CLI flags

CLI flags always win.

---

## Repo config (`.chronikl.toml`)

Drop this in your repo root:

```toml
[provider]
name  = "anthropic"
model = "claude-sonnet-4-6"
# api_key from env (preferred)
# base_url = "..."   # only for openai-compatible / azure / ollama

[voice]
# Pick a bundled profile (`terse` is the default; `prose` for rich
# multi-sentence entries) OR point at a custom Markdown file.
# `path` wins over `profile` if both are set.
profile      = "terse"
# path       = "release-voice.md"
extra_instructions = "Mention the release manager's name."
# Embed truncated commit/PR bodies in the prose-pass user prompt.
# Off by default; turn on when using a richer voice.
rich_context = false

[ladder]
agent_fallback        = false
max_diff_tokens       = 4000
confidence_threshold  = 0.6
batch_size            = 50

[output]
format = "markdown"
# path = "RELEASE_NOTES.md"

[telemetry]
enabled = true

[license]
# key from CHRONIKL_LICENSE_KEY env var (preferred), or via `chronikl license activate`
```

Anything you omit falls back to the built-in default.

## Environment variables

| Variable | Effect |
|---|---|
| `CHRONIKL_PROVIDER` | Provider name (e.g. `anthropic`, `openai-compatible`) |
| `CHRONIKL_MODEL` | Model ID |
| `CHRONIKL_API_KEY` | Generic API key (provider-specific keys also work) |
| `CHRONIKL_BASE_URL` | Base URL for `openai-compatible`, `azure`, `ollama` |
| `CHRONIKL_LICENSE_KEY` | Active license key |
| `CHRONIKL_TELEMETRY` | `false` to disable the heartbeat |
| `CHRONIKL_AUDIT_LOG` | Write a per-run JSON audit log to this path |
| `CHRONIKL_CACHE_DIR` | Override the on-disk cache root |
| `CHRONIKL_NO_PR_ENRICHMENT` | `true` to skip PR fetching even on GitHub |
| `CHRONIKL_DEBUG` | `true` to print the heartbeat payload before posting |
| `GITHUB_TOKEN` / `GH_TOKEN` | Picked up automatically for PR enrichment in CI |

Also accepted: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, etc. — chronikl checks `CHRONIKL_API_KEY` first, then falls back to the provider-specific name.

## Global config

`~/.config/chronikl/config.toml` (or `$XDG_CONFIG_HOME/chronikl/config.toml`) follows the same schema. Repo-local config overrides the global one field-by-field.

## Cache root

By default chronikl caches LLM-derived classifications under:

- `$XDG_CACHE_HOME/chronikl/classifications` (Linux)
- `~/Library/Caches/chronikl/classifications` (macOS)
- Override via `CHRONIKL_CACHE_DIR=...`

```bash
chronikl cache path     # show
chronikl cache stats    # entries + bytes
chronikl cache clear    # wipe
chronikl --no-cache     # bypass for one run
```

## Related Pages

- [LLM Providers](03-Providers) — provider-specific keys
- [CLI Reference](07-CLI-Reference) — every flag
