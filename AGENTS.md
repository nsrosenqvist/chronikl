# AGENTS.md — chronikl

## Project Overview

chronikl is a single-binary CLI that generates release notes from a git
commit range. It is intended to run as part of a release workflow, but also
works locally for previewing notes before tagging.

The core algorithm is a **deterministic-first ladder**: most commits are
classified without an LLM (Conventional Commits prefix, files-only
heuristics, squash-merge PR-ID detection), with optional LLM passes for
ambiguous cases. A final prose pass produces the human-facing Markdown in
the configured voice.

## Architecture

### Module layout

| Module | Purpose |
|---|---|
| `constants/` | Single source of truth: app name, env var names, URLs, defaults |
| `env/` | Testable env-var wrapper |
| `http/` | Centralized `reqwest::Client` factory |
| `ci/` | CI provider detection (GitHub/GitLab/Bitbucket/…) |
| `cli/` | clap-derive argument parsing |
| `config/` | TOML + env + flag layering |
| `git/` | Range resolution, commit log parsing, Conventional Commits, merge-style detection |
| `enrichment/` | `PrEnricher` trait + GitHub (octocrab) impl + no-op fallback |
| `ladder/` | Tier 0 (deterministic) → 1 (batched LLM) → 2 (per-commit + diff) → 3 (agentic, opt-in) |
| `prose/` | Final prose pass: voice + section assembly |
| `voice/` | Voice file loading (frontmatter+body); bundled default via `include_str!` |
| `providers/` | `NotesProvider` trait + rig-core multi-provider impl |
| `output/` | Markdown rendering, optional changelog prepend |
| `cache/` | SHA-keyed disk cache for classifications |
| `license/` | Ed25519 offline license verification (ported from nitpik) |
| `update/` | Self-update from GitHub releases (ported from nitpik) |
| `telemetry/` | Anonymous fire-and-forget heartbeat per run; opt-out via `--no-telemetry` |
| `models/` | `Commit`, `EnrichedCommit`, `Classification`, `Section`, `Release` |
| `progress/` | CI-aware progress reporter |

### Key principles

- All inter-module communication goes through `models/`.
- `main.rs` is the composition root — wires everything together.
- Async-first (tokio).
- `constants.rs` is the single source of truth for names and URLs.

## Conventions

### Git

- Conventional commits
- Imperative mood, <72 char subject line.
- One logical change per commit.

### Error handling

- Library code uses `thiserror` for typed error enums.
- The CLI binary boundary uses `anyhow` with `.context()` chains.

### Naming

- Snake_case for files and modules; PascalCase for types.
- American English throughout.

### Spelling, formatting, linting

- `cargo fmt --all`
- `cargo clippy --all-targets -- -D warnings`

### Dependencies

Keep lean; justify additions in commit messages. Notable runtime deps:

- `rig-core` — multi-provider LLM abstraction
- `octocrab` — GitHub API for PR enrichment
- `clap` — CLI parsing
- `tokio` + `reqwest` — async + HTTP
- `ed25519-dalek` — offline license verification

### Configuration & environment variables

Precedence (highest wins):

1. CLI flags
2. Environment variables (`CHRONIKL_*` prefix)
3. `.chronikl.toml` in repo root
4. `~/.config/chronikl/config.toml` (global)
5. Built-in defaults

Provider API keys fall back to provider-specific names
(`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, etc.) when `CHRONIKL_API_KEY` is
unset.

## Verification before finishing a task

Run the following before declaring a task complete. All must pass:

1. `cargo fmt --all -- --check`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo test`
4. `cargo build`

If any step fails, fix the underlying issue rather than skipping or
suppressing the check. If a change cannot be verified end-to-end
(e.g. a UI or integration path that is not exercisable in this
environment), state that explicitly instead of claiming success.
