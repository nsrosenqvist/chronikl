# LLM Providers

chronikl uses [rig-core](https://crates.io/crates/rig-core) to talk to LLMs. Bring your own provider and API key.

---

## Supported providers

| Provider | `CHRONIKL_PROVIDER` value | API key env var |
|---|---|---|
| Anthropic | `anthropic` | `ANTHROPIC_API_KEY` |
| OpenAI | `openai` | `OPENAI_API_KEY` |
| Azure (OpenAI) | `azure` | `AZURE_OPENAI_API_KEY` (also requires `CHRONIKL_BASE_URL`) |
| Cohere | `cohere` | `COHERE_API_KEY` |
| DeepSeek | `deepseek` | `DEEPSEEK_API_KEY` |
| Galadriel | `galadriel` | `CHRONIKL_API_KEY` |
| Gemini (Google) | `gemini` | `GEMINI_API_KEY` |
| GitHub Models | `github-models` | `GITHUB_MODELS_TOKEN` or `GITHUB_TOKEN` — needs `models:read` scope |
| Groq | `groq` | `GROQ_API_KEY` |
| HuggingFace | `huggingface` | `HF_API_KEY` |
| Hyperbolic | `hyperbolic` | `CHRONIKL_API_KEY` |
| Mira | `mira` | `CHRONIKL_API_KEY` |
| Mistral | `mistral` | `MISTRAL_API_KEY` |
| Moonshot | `moonshot` | `MOONSHOT_API_KEY` |
| Ollama (local) | `ollama` | none — set `CHRONIKL_BASE_URL` |
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY` |
| Perplexity | `perplexity` | `PERPLEXITY_API_KEY` |
| Together AI | `together` | `TOGETHER_API_KEY` |
| xAI | `xai` | `XAI_API_KEY` |
| OpenAI-compatible (custom) | `openai-compatible` | `CHRONIKL_API_KEY` (requires `CHRONIKL_BASE_URL`) |

All providers also accept `CHRONIKL_API_KEY` as a fallback, and provider-specific keys can be set under their well-known names above.

## Picking a model

```bash
export CHRONIKL_PROVIDER=anthropic
export CHRONIKL_MODEL=claude-sonnet-4-6
export CHRONIKL_API_KEY=sk-ant-...
```

There's no default model — you must set `CHRONIKL_MODEL` (or `[provider] model = "..."` in `.chronikl.toml`). Recommended starting points:

- **Anthropic**: `claude-haiku-4-5` (cheap classification), `claude-sonnet-4-6` (prose pass), `claude-opus-4-7` (long releases or polished prose)
- **OpenAI**: `gpt-5-mini` (cheap), `gpt-5` (prose)
- **Gemini**: `gemini-2.5-flash` (cheap), `gemini-2.5-pro` (prose)

The same model is used for all tiers. For most release ranges (under ~50 commits) a mid-tier model gives good results.

## Custom OpenAI-compatible endpoints

Useful for self-hosted LLMs (vLLM, LiteLLM, Together's compat API, LMStudio, etc.):

```bash
export CHRONIKL_PROVIDER=openai-compatible
export CHRONIKL_BASE_URL=https://my-llm.example.com/v1
export CHRONIKL_MODEL=my-model-id
export CHRONIKL_API_KEY=sk-internal-...
```

## Local Ollama

```bash
export CHRONIKL_PROVIDER=ollama
export CHRONIKL_BASE_URL=http://localhost:11434
export CHRONIKL_MODEL=llama3
```

No API key required. Ollama doesn't support strict structured outputs as well as Anthropic/OpenAI, so chronikl's response parser is tolerant of markdown-fenced or prose-prefixed JSON.

## Data sent to the provider

Every chronikl run sends content to your configured provider. Choose a provider you trust, or run a local model via Ollama. What crosses the boundary:

- **Always:** commit messages (subject + body) for every commit in the range; file paths touched; the configured voice prompt and any `--prompt` addendum.
- **Tier 2:** the commit diff (truncated to `max_diff_tokens`, default 4000) for any commit Tier 1 flagged as low-confidence.
- **Tier 3 (`--agent`):** whatever files the agent's tools read during its turns — by default a sandboxed view of the repo. Only fires when `--agent` is on.
- **PR enrichment:** PR titles, bodies, and labels for the PRs linked to commits in range, when a `GITHUB_TOKEN` is available.
- **`--rich-context`:** full commit bodies and PR bodies in the prose-pass user prompt (off by default).

What does **not** cross the boundary: repo files outside the diff (without `--agent`), environment variables, your license key, or unrelated git history.

If any of this is sensitive — diffs containing customer data, internal-only PR bodies, etc. — pick a provider with an appropriate DPA, or use Ollama to keep everything on-device.

## Cost control

The classification ladder is cost-aware:

- **Tier 0** runs deterministically with no LLM calls (Conventional Commits + files-only heuristics).
- **Tier 1** batches 50 commits per call by default — minimises overhead on large releases.
- **Tier 2** only fires for commits Tier 1 was uncertain about (`confidence < 0.6`).
- **Tier 3** (`--agent`) is opt-in and runs per commit with read tools.
- **Prose pass** is one call regardless of release size.

A 50-commit release on `claude-sonnet-4-6` typically uses 5,000–15,000 input tokens and 1,000–3,000 output tokens, depending on diff size and how many commits need Tier 2.

## Related Pages

- [GitHub Models](04-GitHub-Models) — hosted OpenAI-compatible gateway, zero-secret setup in GitHub Actions
- [Configuration](05-Configuration) — TOML alternatives to env vars
- [CLI Reference](08-CLI-Reference) — `--provider`, `--model`, `--api-key`, `--base-url`
