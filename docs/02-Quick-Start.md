# Quick Start

Generate AI-written release notes from your git history in three steps.

---

## 1. Install chronikl

```bash
curl -sSfL https://raw.githubusercontent.com/nsrosenqvist/chronikl/main/install.sh | sh
```

See [Installation](01-Installation) for other options (Docker, Homebrew, crates.io).

## 2. Connect an LLM provider

Set two environment variables — a provider name and the corresponding API key:

```bash
export CHRONIKL_PROVIDER=anthropic
export CHRONIKL_MODEL=claude-sonnet-4-6
export CHRONIKL_API_KEY=sk-ant-...
```

chronikl supports Anthropic, OpenAI, Gemini, Cohere, DeepSeek, xAI, Groq, Perplexity, Ollama, and any OpenAI-compatible endpoint. See [LLM Providers](03-Providers).

## 3. Generate notes

From your repository, just run `chronikl`:

```bash
chronikl
```

By default chronikl finds the latest semver tag, diffs against `HEAD`, and writes Markdown to stdout:

```
info: PR enrichment via GitHub for nsrosenqvist/myproject (authenticated)
info: enriched 5 commit(s) with PR metadata
info: version bump: minor (semver)
info: Tier 1 reclassified 3 commit(s)

### Features

- Add SSO login support (#42)
- Surface request IDs in API responses (#45)

### Bug Fixes

- Fix race in cache invalidation (#43)
- Recover from missing config file gracefully (#46)
```

To explicit a range:

```bash
chronikl --from v0.1.0 --to v0.2.0
```

To write to a file:

```bash
chronikl --output release-notes.md
```

To preview without calling the LLM (Tier 0 deterministic only):

```bash
chronikl --no-llm
```

## What's next?

- **Set a voice** — give chronikl your own Markdown file with prose style. See [Voice](05-Voice).
- **Drop a config** — write a `.chronikl.toml` so the team doesn't need to set env vars each run. See [Configuration](04-Configuration).
- **Run in CI** — fire chronikl from your release workflow and feed the output to `softprops/action-gh-release`. See [CI Integration](06-CI-Integration).
- **Enable agentic mode** — `--agent` lets the LLM read files and search the repo to figure out cryptic commits. See [CLI Reference](07-CLI-Reference).

## Related Pages

- [Installation](01-Installation) — all install methods
- [LLM Providers](03-Providers) — provider setup
- [CLI Reference](07-CLI-Reference) — every flag
