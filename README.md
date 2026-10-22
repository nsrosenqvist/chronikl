# chronikl

[![CI](https://github.com/nsrosenqvist/chronikl/workflows/CI/badge.svg)](https://github.com/nsrosenqvist/chronikl/actions)
[![Crates.io](https://img.shields.io/crates/v/chronikl)](https://crates.io/crates/chronikl)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](#license)

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
- **Voice control.** Pick a bundled profile (`--voice terse` for one-line bullets, `--voice prose` for richer multi-sentence entries, `--voice simple` for the product's users) or point at your own plain-Markdown voice file. Add `--rich-context` to feed commit + PR bodies to the prose pass; `--prompt "..."` for one-off tweaks.
- **Notes for users too.** `--voice simple` writes a short plain-text "What's new" for app stores: only what users can see or do, flat, capped to fit, and a friendly fallback line when a release holds nothing user-facing. A voice file's frontmatter can carry the same rendering settings (`headings`, `exclude_sections`, `max_chars`, `format`, …), and flags or `[render]` override them. `--strict` fails the run instead of falling back to deterministic notes when the model can't be reached.
- **Release-context aware.** Detects prerelease vs. stable, semver vs. CalVer, and the bump kind (major/minor/patch). Prose adjusts tone — major releases lead with breaking changes, patches lead with fixes, etc.
- **Veritrail-style audit log.** Optional `--audit-log` writes a full JSON record of every LLM call (model, tokens, prompt + response hashes, tool calls). Lets you answer "why did chronikl say that?" months later.
- **20 LLM providers.** Anthropic, OpenAI, Gemini, Cohere, DeepSeek, xAI, Groq, Perplexity, HuggingFace, Mistral, Moonshot, Ollama, Azure, OpenRouter, Together, and any OpenAI-compatible endpoint.
- **CI-first.** Ships as a GitHub Action, install.sh, Dockerfile, and self-update binary. Designed to drop into a release workflow.

## Quick install

```bash
curl -sSfL https://raw.githubusercontent.com/nsrosenqvist/chronikl/main/install.sh | sh
```

Or via Homebrew, Cargo, or Docker — see the [Installation guide](https://chronikl.dev/docs/installation/).

## Quick start

```bash
export CHRONIKL_PROVIDER=anthropic
export CHRONIKL_MODEL=claude-sonnet-4-6
export CHRONIKL_API_KEY=sk-ant-...

cd path/to/your/repo
chronikl
```

That's it. chronikl finds the latest semver tag, diffs against `HEAD`, classifies, and writes Markdown to stdout. See the [Quick Start](https://chronikl.dev/docs/quick-start/) for more.

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

See [CI Integration](https://chronikl.dev/docs/ci-integration/) for more.

## Documentation

Full docs live at [chronikl.dev/docs](https://chronikl.dev/docs):

- [Installation](https://chronikl.dev/docs/installation/)
- [Quick Start](https://chronikl.dev/docs/quick-start/)
- [LLM Providers](https://chronikl.dev/docs/providers/)
- [GitHub Models](https://chronikl.dev/docs/github-models/) (retired by GitHub on 2026-07-30)
- [Configuration](https://chronikl.dev/docs/configuration/)
- [Voice](https://chronikl.dev/docs/voice/)
- [CI/CD Integration](https://chronikl.dev/docs/ci-integration/)
- [CLI Reference](https://chronikl.dev/docs/cli-reference/)
- [Troubleshooting](https://chronikl.dev/docs/troubleshooting/)

The docs source lives in [`docs/`](docs/); the published site is built from each tagged release.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

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
