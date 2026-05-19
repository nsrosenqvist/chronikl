# chronikl

[![CI](https://github.com/nsrosenqvist/chronikl/workflows/CI/badge.svg)](https://github.com/nsrosenqvist/chronikl/actions)
[![Crates.io](https://img.shields.io/crates/v/chronikl)](https://crates.io/crates/chronikl)
[![License](https://img.shields.io/badge/license-BUSL--1.1-blue)](LICENSE)

**AI-powered release notes for your team.** Generate clean, sectioned, voice-tuned Markdown release notes from your git history. Bring your own model, bring your own API key.

```
$ chronikl --from v0.1.0 --to v0.2.0

### Features
- Add SSO login support (#42)
- Surface request IDs in API responses (#45)

### Bug Fixes
- Fix race in cache invalidation (#43)
- Recover from missing config file gracefully (#46)
```

---

## Why chronikl?

- **Cost-aware classification ladder.** Tier 0 (deterministic, no LLM) → Tier 1 (batched LLM) → Tier 2 (per-commit + diff) → Tier 3 (optional agentic). Most commits never reach an LLM. A 50-commit release typically uses a few thousand tokens.
- **PR enrichment.** When run on a GitHub repo with a token, chronikl fetches PR titles, bodies, and labels and feeds them to the LLM — much better signal than raw commit messages.
- **Voice control.** Pick a bundled profile (`--voice terse` for one-line bullets, `--voice prose` for richer multi-sentence entries) or point at your own plain-Markdown voice file. Add `--rich-context` to feed commit + PR bodies to the prose pass; `--prompt "..."` for one-off tweaks.
- **Release-context aware.** Detects prerelease vs. stable, semver vs. CalVer, and the bump kind (major/minor/patch). Prose adjusts tone — major releases lead with breaking changes, patches lead with fixes, etc.
- **Veritrail-style audit log.** Optional `--audit-log` writes a full JSON record of every LLM call (model, tokens, prompt + response hashes, tool calls). Lets you answer "why did chronikl say that?" months later.
- **20 LLM providers.** Anthropic, OpenAI, Gemini, GitHub Models, Cohere, DeepSeek, xAI, Groq, Perplexity, HuggingFace, Mistral, Moonshot, Ollama, Azure, OpenRouter, Together, and any OpenAI-compatible endpoint.
- **CI-first.** Ships as a GitHub Action, install.sh, Dockerfile, and self-update binary. Designed to drop into a release workflow.

## Quick install

```bash
curl -sSfL https://raw.githubusercontent.com/nsrosenqvist/chronikl/main/install.sh | sh
```

Or via Homebrew, Cargo, or Docker — see the [Installation guide](https://github.com/nsrosenqvist/chronikl/wiki/01-Installation).

## Quick start

```bash
export CHRONIKL_PROVIDER=anthropic
export CHRONIKL_MODEL=claude-sonnet-4-6
export CHRONIKL_API_KEY=sk-ant-...

cd path/to/your/repo
chronikl
```

That's it. chronikl finds the latest semver tag, diffs against `HEAD`, classifies, and writes Markdown to stdout. See the [Quick Start](https://github.com/nsrosenqvist/chronikl/wiki/02-Quick-Start) for more.

## In a release workflow

```yaml
# .github/workflows/release.yml
on:
  push:
    tags: ["v[0-9]+.[0-9]+.[0-9]+*"]

jobs:
  release:
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v6
        with: { fetch-depth: 0 }

      - uses: nsrosenqvist/chronikl@v1
        with:
          output: release_notes.md
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          CHRONIKL_API_KEY: ${{ secrets.CHRONIKL_API_KEY }}

      - uses: softprops/action-gh-release@v2
        with:
          body_path: release_notes.md
```

See [CI Integration](https://github.com/nsrosenqvist/chronikl/wiki/06-CI-Integration) for more.

## Documentation

Full docs live in the [GitHub Wiki](https://github.com/nsrosenqvist/chronikl/wiki):

- [Installation](https://github.com/nsrosenqvist/chronikl/wiki/01-Installation)
- [Quick Start](https://github.com/nsrosenqvist/chronikl/wiki/02-Quick-Start)
- [LLM Providers](https://github.com/nsrosenqvist/chronikl/wiki/03-Providers)
- [Configuration](https://github.com/nsrosenqvist/chronikl/wiki/04-Configuration)
- [Voice](https://github.com/nsrosenqvist/chronikl/wiki/05-Voice)
- [CI/CD Integration](https://github.com/nsrosenqvist/chronikl/wiki/06-CI-Integration)
- [CLI Reference](https://github.com/nsrosenqvist/chronikl/wiki/07-CLI-Reference)
- [Licensing](https://github.com/nsrosenqvist/chronikl/wiki/08-Licensing)
- [Troubleshooting](https://github.com/nsrosenqvist/chronikl/wiki/09-Troubleshooting)

The `docs/` folder in this repo is auto-synced to the wiki on each push to `main`.

## License

[BUSL-1.1](LICENSE). Free for personal, educational, non-commercial, and open-source use. Commercial production use requires a separate license — contact `niklas.s.rosenqvist@gmail.com`. Each version converts to Apache-2.0 three years after release.

See [Licensing](https://github.com/nsrosenqvist/chronikl/wiki/08-Licensing) for activation details.

## Contributing

See [AGENTS.md](AGENTS.md) for architecture and module-layout conventions.

```bash
cargo nextest run --all-targets        # full test suite
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

E2E tests against a real LLM are gated by `#[ignore]`:

```bash
ANTHROPIC_API_KEY=sk-ant-... cargo test --test e2e_profiles -- --ignored --nocapture
```
