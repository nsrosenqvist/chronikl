# Troubleshooting

---

## "fatal: ambiguous argument 'v0.1.0..HEAD'"

You passed a `--from` ref that doesn't exist as a tag/commit in the repo. Run `git tag --list 'v*.*.*' --sort=-v:refname` to see what's available, or use `chronikl --auto` (the default) to let chronikl pick the previous tag.

In CI, this usually means `actions/checkout` ran with the default `fetch-depth: 1`. Set `fetch-depth: 0` so the runner has all tags.

## "no API key resolved for provider 'anthropic'"

Set `ANTHROPIC_API_KEY` (or `CHRONIKL_API_KEY`) in your env. See [Providers](03-Providers) for the per-provider key names.

## "PR enrichment skipped (no GitHub remote detected)"

Expected on non-GitHub forges (GitLab, Bitbucket, self-hosted Gitea/Forgejo, …). Notes still get generated; PR titles/labels just aren't pulled in.

If you *are* on GitHub but seeing this, check that `git remote get-url origin` returns a `github.com` URL.

## "PR enrichment via GitHub for … (anonymous (60 req/hour limit))"

You're using anonymous GitHub API access. Fine for small repos; on larger ones you'll hit the rate limit. Set `GITHUB_TOKEN` (auto-injected in GitHub Actions) or a personal `GH_TOKEN` for 5,000 req/hour.

## All commits land in "Other"

This means Tier 0 didn't recognise any Conventional Commits prefixes and Tier 1+ is either disabled (`--no-llm`) or the LLM didn't engage. Check:

- Is `CHRONIKL_PROVIDER` + API key set?
- Is `--no-llm` accidentally set?
- Run `chronikl debug classify` to see Tier 0 results
- Run `chronikl debug prompts` to see what would be sent if the LLM ran

## "LLM API error: HTTP 429 Too Many Requests"

You're being rate-limited by your provider. chronikl retries transient errors automatically (up to 5 times with exponential backoff capped at 60s). If you hit this consistently, batch_size in `[ladder]` is too large — try `batch_size = 25` to reduce per-call payload size, or use a different model.

## "could not parse LLM response as JSON"

The model returned malformed structured output. chronikl already tolerates markdown-fenced JSON, prose-prefixed JSON, and bare JSON arrays. If you're hitting this consistently:

- Try a more capable model (Sonnet/Opus over Haiku)
- For OpenAI-compatible / Ollama endpoints, ensure the runtime supports JSON-mode or constrained decoding

## "warn: prose pass failed; falling back to deterministic render: …"

The prose pass errored (rate limit, timeout, model issue). The deterministic Tier 0 → Markdown render kicks in so you still get notes — they just won't have the voice applied. Re-run later, or use `chronikl debug prompts` to inspect what was about to be sent.

## "permission denied replacing /usr/local/bin/chronikl"

`chronikl update` couldn't replace the running binary. Run `sudo chronikl update`, or install to a user-writable location (`CHRONIKL_INSTALL_DIR=~/.local/bin`).

## Tier 3 agentic mode never finishes

The agent loop has a default cap of 10 turns. If a model gets stuck in tool-call loops, the loop returns the partial state. Check the audit log for `terminated_via_tool: null` and a high `turns` count — that's the signal. Try a more capable model, or skip Tier 3 (drop `--agent`).

## My CalVer tag isn't recognised

chronikl recognises CalVer in two shapes:

- 4-digit year ≥ 2000: `2024.05.08`, `v2024.10`
- 2-digit year (10–99): `24.05.08`, `v24.5`

Year–month–day structures with valid month (1–12) and optional day (1–99). If yours doesn't fit (e.g. `release-2024-05-08`, dashes instead of dots), chronikl returns `version_bump: unknown` and uses neutral framing in the prose pass.

## Where do I see exactly what got sent to the LLM?

```bash
chronikl debug prompts --from <REF> --to <REF>
```

Dumps every prompt (Tier 1 / Tier 2 / Tier 3 / prose) that would be built, without making any LLM calls.

For a real run, set `--audit-log audit.json` — every LLM call is recorded with prompt + response SHA-256 hashes, token counts, and the full classification trail.

## Telemetry — what gets sent?

chronikl posts an anonymous heartbeat per run with aggregate counters only: commit count, tier counts, model name (not key), provider name, success/fail, version. No commit text, no prose, no diffs, no API keys, no file paths.

Disable with `--no-telemetry`, `CHRONIKL_TELEMETRY=false`, or `[telemetry] enabled = false` in `.chronikl.toml`.

To inspect what *would* be sent:

```bash
CHRONIKL_DEBUG=1 chronikl …
```

Prints the full payload to stderr before posting.

## Related Pages

- [CLI Reference](08-CLI-Reference) — every flag
- [Configuration](05-Configuration) — env vars + TOML
