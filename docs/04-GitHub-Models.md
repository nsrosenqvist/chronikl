# GitHub Models

[GitHub Models](https://docs.github.com/en/github-models) is GitHub's hosted, OpenAI-compatible inference gateway. It exposes OpenAI, Meta Llama, DeepSeek, xAI, Microsoft Phi, and other publishers behind a single endpoint, authenticates with a GitHub token (PAT or the auto-injected `GITHUB_TOKEN`), and is included with paid Copilot tiers (a small free tier is available without Copilot).

For chronikl this is the lowest-friction provider in CI: the `GITHUB_TOKEN` your release workflow already has is also the API key. No extra secret to manage.

---

## Setup

### GitHub Actions (zero extra secrets)

```yaml
jobs:
  release:
    runs-on: ubuntu-latest
    permissions:
      contents: write
      models: read        # required for GitHub Models
    steps:
      - uses: actions/checkout@v6
        with:
          fetch-depth: 0

      - uses: nsrosenqvist/chronikl@v0
        with:
          output: release_notes.md
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          CHRONIKL_PROVIDER: github-models
          CHRONIKL_MODEL: openai/gpt-4o-mini

      - uses: softprops/action-gh-release@v2
        with:
          body_path: release_notes.md
```

The `models: read` permission is the only thing CI workflows usually need to add — `GITHUB_TOKEN` is already injected by Actions and chronikl picks it up automatically.

### Local dev (CLI)

Create a fine-grained PAT with the **Models: Read** account permission at `https://github.com/settings/personal-access-tokens`, or use the `gh` CLI token:

```bash
export CHRONIKL_PROVIDER=github-models
export CHRONIKL_MODEL=openai/gpt-4o-mini
export GITHUB_TOKEN=$(gh auth token)         # or a dedicated fine-grained PAT
chronikl --auto --output release_notes.md
```

To keep models access separate from your general `GITHUB_TOKEN`, use `GITHUB_MODELS_TOKEN` — chronikl checks it first.

### Org-attributed billing

To bill usage to a specific organisation rather than the calling user, override the base URL:

```bash
export CHRONIKL_BASE_URL=https://models.github.ai/orgs/<org>/inference
```

Everything else stays the same.

---

## Picking a model

GitHub Models identifiers are `{publisher}/{name}`. Some starting points:

- `openai/gpt-4o-mini` — cheap and fast, good for Tier 1 classification on most repos
- `openai/gpt-4.1` — stronger prose pass
- `meta/Llama-3.3-70B-Instruct` — open-weights alternative
- `deepseek/DeepSeek-V3` — strong reasoning at low cost
- `xai/grok-3` — when you want grok-flavoured prose

The catalog moves quickly. The live list with capability notes is at [github.com/marketplace/models](https://github.com/marketplace/models).

---

## Rate limits

Free tier limits are tight and reset daily; paid Copilot tiers raise the ceilings substantially (see [GitHub's rate-limit docs](https://docs.github.com/en/github-models)).

Chronikl's ladder is designed to be call-cheap:

- **Tier 0** runs deterministically with no LLM call.
- **Tier 1** batches up to 50 commits per request.
- **Tier 2** only fires for low-confidence Tier 1 verdicts.
- **Prose pass** is one call regardless of release size.

A 50-commit release on `openai/gpt-4o-mini` usually issues 2–6 LLM calls total — well inside the free tier. `--agent` mode (Tier 3) fires per uncertain commit and can blow the daily cap on big releases; for that workload pick a paid Copilot tier or a regular OpenAI/Azure key.

429s with `Retry-After` are handled by chronikl's retry layer (`MAX_RETRIES=5`, exponential backoff up to 60s).

---

## Caveats

> **Third-party dependency notice:** GitHub Models is a separate service operated by GitHub, not by chronikl. Available models, rate limits, model name format, and request semantics are all subject to change by GitHub at any time. Before depending on a specific model for production CI, verify it works with the free unlicensed version of chronikl.

- **Pin a model.** GitHub may deprecate or rename models without notice; set an explicit `CHRONIKL_MODEL` (in `.chronikl.toml` or env) rather than relying on a vendor alias.
- **Schema strictness varies by underlying model.** OpenAI publisher models honour `response_format: json_schema`; some others don't. Chronikl's response parser tolerates markdown-fenced or prose-prefixed JSON, so this rarely matters in practice.
- **Prompt caching is opaque.** GitHub Models doesn't expose a caching opt-in header (unlike Anthropic via rig's `with_automatic_caching()`) and upstream caching behaviour isn't documented. Chronikl's audit log may not report cache hits even when the underlying provider is caching.
- **Free tier resets per UTC day.** Plan large releases accordingly — a 200-commit release with `--agent` started late in the UTC day may exhaust the daily cap mid-run.
- **PAT scope drift.** GitHub may rename `models:read` over time. If you get a 403, re-issue the PAT and check the latest scope name in GitHub's settings UI.

---

## Related Pages

- [LLM Providers](03-Providers) — the full provider matrix and config-precedence model
- [Configuration](05-Configuration) — TOML alternatives to env vars
- [CI/CD Integration](07-CI-Integration) — full release-workflow examples
- [CLI Reference](08-CLI-Reference) — `--provider`, `--model`, `--base-url`
