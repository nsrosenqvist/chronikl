# Configuration

chronikl reads from these layers in order of increasing precedence:

1. Built-in defaults (for `[render]`: then the voice's frontmatter)
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
# Sampling temperature: a number (0–2) or "none" to send none. Default 0.
# temperature = "none"

[voice]
# Pick a bundled profile (`terse` is the default; `prose` for rich
# multi-sentence entries; `simple` for the product's users) OR point at a custom
# Markdown file (relative paths resolve against the repo root).
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

[render]
# Any of these also work in a voice file's frontmatter; values here win.
# headings         = true
# include_sections = ["features", "fixes"]
# exclude_sections = ["tests", "docs", "ci"]
# max_bullets      = 5
# max_chars        = 450
# footer           = true
# format           = "markdown"     # or "plain"
# user_facing      = false
# fallback_line    = "Small improvements behind the scenes."
# fallback_fixes / fallback_performance / fallback_other = "..."

[output]
# path = "RELEASE_NOTES.md"
# Exit non-zero, writing nothing, when the notes would fall back to the
# deterministic renderer.
# strict = false
```

Anything you omit falls back to the built-in default.

## Rendering (`[render]`)

`[render]` decides what the notes look like and who they're for. The same keys can sit in a voice file's frontmatter (see [Voice](06-Voice#frontmatter)), so a voice carries its own shape: the bundled `simple` voice sets them for app-store notes, while `terse` and `prose` set none and keep chronikl's classic output. Every key also has a CLI flag and an environment variable.

Precedence, highest first, key by key:

1. CLI flags
2. Environment variables
3. `.chronikl.toml` `[render]` (repo), then the global config's
4. The voice's frontmatter
5. Built-in defaults

| Key | Flag | Env | Default | Effect |
|---|---|---|---|---|
| `headings` | `--headings[=BOOL]` | `CHRONIKL_HEADINGS` | `true` | `false` renders one flat bullet list, no section headings or overview. |
| `include_sections` | `--include-sections` | `CHRONIKL_INCLUDE_SECTIONS` | all | Keep only these sections. Set above the voice, it replaces the voice's `exclude_sections` unless you set those too. |
| `exclude_sections` | `--exclude-sections` | `CHRONIKL_EXCLUDE_SECTIONS` | none | Commits in these sections never reach the prose pass or the output. An empty list clears the voice's. |
| `max_bullets` | `--max-bullets` | `CHRONIKL_MAX_BULLETS` | none | Keep at most this many bullets. |
| `max_chars` | `--max-chars` | `CHRONIKL_MAX_CHARS` | none | Keep the whole output (footer included, final line break not) within this many characters. |
| `footer` | `--footer[=BOOL]` | `CHRONIKL_FOOTER` | `true` | `false` drops the `**Full Changelog**` compare link and any trailer the model wrote. |
| `format` | `--format` | `CHRONIKL_FORMAT` | `markdown` | `plain`: `- ` bullets, no bold, italics, code spans, links or `#` headings. |
| `user_facing` | `--user-facing[=BOOL]` | `CHRONIKL_USER_FACING` | `false` | Notes for the product's users (see below). |
| `fallback_line` | `--fallback-line` | `CHRONIKL_FALLBACK_LINE` | — | "Nothing user-facing" line for any range. |
| `fallback_fixes` | `--fallback-fixes` | `CHRONIKL_FALLBACK_FIXES` | `Bug fixes and stability improvements.` | … for a range with bug fixes. |
| `fallback_performance` | `--fallback-performance` | `CHRONIKL_FALLBACK_PERFORMANCE` | `Faster and smoother in places.` | … for a range with performance work and no fixes. |
| `fallback_other` | `--fallback-other` | `CHRONIKL_FALLBACK_OTHER` | `Small improvements behind the scenes.` | … for any other range (docs, tests, CI, internal features, an empty range). |

Section ids: `breaking`, `features`, `fixes` (or `bug-fixes`), `performance` (`perf`), `refactor`, `docs` (`documentation`), `tests`, `build`, `ci`, `chore`, `other`. In env vars, separate them with commas; an empty `CHRONIKL_EXCLUDE_SECTIONS=` clears the list.

**Limits** cut at whole bullets, from the end, so the model's order (most noticeable first) decides what stays. A bullet is never cut mid-text. When the output is still too long, the footer goes first, then bullets from the end, then any other paragraphs. The prose prompt states the same limits, so the model usually fits on its own.

The prose model is asked for exactly this shape, whatever its source, and chronikl enforces it on whatever comes back (and on the deterministic fallback).

### User-facing notes

`user_facing = true` (the `simple` voice sets it) writes for the product's users:

- The prose pass is told to keep only changes a user can see or do, in plain words, and to leave out internal and staff-only items (admin tools, test or review accounts) even when they were classified as features or fixes. The bump and prerelease framings, which describe a developer document, are left out.
- The output is never empty or a raw commit subject. When nothing in the range is user-facing — every commit is in an excluded section, or the model finds nothing a user would notice — chronikl writes one line instead, picked from the range's classified sections: any bug fix → `fallback_fixes`; otherwise any performance work → `fallback_performance`; otherwise → `fallback_other`. A per-kind line wins over `fallback_line`, which wins over the built-in line. The choice is deterministic, so it works without the model too.
- When the model can't be reached, the same line stands in for the commit subjects the deterministic renderer would otherwise print.

A range with some user-facing items and some internal ones shows only the user-facing ones; no fallback line. The audit log's `notes_source` says `nothing_user_facing` (with the `fallback` kind) when the line was written because nothing was user-facing; `--strict` accepts that.

## Temperature

chronikl sends `temperature = 0` with every call so classification and prose are as repeatable as the model allows. Some models reject the parameter (Anthropic's Claude 5.5 family: "temperature is deprecated for this model"; OpenAI's reasoning models). When a call is rejected for it, chronikl logs a warning, retries that call once without it, and leaves it out for the rest of the run. The same goes for OpenAI models that want `max_completion_tokens` instead of `max_tokens`. The audit log marks the call that changed (`param_fallbacks`).

Set `[provider] temperature`, `CHRONIKL_TEMPERATURE` or `--temperature` to a number, or to `none` to never send one.

## Strict mode

By default, when the LLM pipeline can't produce prose (no provider or key, a provider that can't be built, a failed prose pass), chronikl writes notes from its deterministic renderer (or, for user-facing notes, the fallback line) and exits 0. With `--strict` (`CHRONIKL_STRICT=true`, `[output] strict = true`) such a run exits 1 with a message saying why, and writes no notes (stdout and `--output` stay empty; `--audit-log` is still written). `--no-llm` asks for the deterministic renderer, so strict mode accepts it.

The audit log's `notes_source` records where the notes came from: `model`, `fallback` (with the reason), `deterministic`, or `nothing_user_facing` (with the `fallback` kind). Only `fallback` fails a strict run; a user-facing range with nothing user-facing in it is a normal outcome.

## Environment variables

| Variable | Effect |
|---|---|
| `CHRONIKL_PROVIDER` | Provider name (e.g. `anthropic`, `openai-compatible`) |
| `CHRONIKL_MODEL` | Model ID |
| `CHRONIKL_API_KEY` | Generic API key (provider-specific keys also work) |
| `CHRONIKL_BASE_URL` | Base URL for `openai-compatible`, `azure`, `ollama` |
| `CHRONIKL_AUDIT_LOG` | Write a per-run JSON audit log to this path |
| `CHRONIKL_CACHE_DIR` | Override the on-disk cache root |
| `CHRONIKL_NO_PR_ENRICHMENT` | `true` to skip PR fetching even on GitHub |
| `CHRONIKL_TEMPERATURE` | Sampling temperature, or `none` |
| `CHRONIKL_STRICT` | `true` to fail instead of falling back to deterministic notes |
| `CHRONIKL_HEADINGS`, `CHRONIKL_INCLUDE_SECTIONS`, `CHRONIKL_EXCLUDE_SECTIONS`, `CHRONIKL_MAX_BULLETS`, `CHRONIKL_MAX_CHARS`, `CHRONIKL_FOOTER`, `CHRONIKL_FORMAT`, `CHRONIKL_USER_FACING`, `CHRONIKL_FALLBACK_LINE`, `CHRONIKL_FALLBACK_FIXES`, `CHRONIKL_FALLBACK_PERFORMANCE`, `CHRONIKL_FALLBACK_OTHER` | Rendering, see above |
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
- [CLI Reference](08-CLI-Reference) — every flag
