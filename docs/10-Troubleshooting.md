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

This means Tier 0 didn't recognise any Conventional Commits prefixes and Tier 1+ is either disabled (`--no-llm`) or the LLM didn't engage. Subjects written as `Area: text` (`Map: …`, `Web: …`) aren't Conventional types chronikl knows, so they go to Tier 1 like free-form subjects; without an LLM they stay in "Other". Check:

- Is `CHRONIKL_PROVIDER` + API key set?
- Is `--no-llm` accidentally set?
- Run `chronikl debug classify` to see Tier 0 results
- Run `chronikl debug prompts` to see what would be sent if the LLM ran

## User-facing changes filed under Documentation or Tests

The classifiers decide by what a change does for users, not by which files it touched: a feature that also adds tests or docs is a feature, and product copy (UI text, a website's FAQ) isn't documentation. A `tests` or `documentation` verdict for a commit that also changes product files is turned into `other`. If a commit still lands in the wrong section, check what the message says it does: the classifier reads the subject and body. `chronikl debug prompts` shows the Tier 1 input, files listed product-first with a count by kind.

Repos that pin `confidence_threshold = 0.0` in `[ladder]` never send commits to Tier 2 (the per-commit, diff-aware pass), so Tier 1's call is final.

## "temperature is deprecated for this model" / "Unsupported parameter: 'max_tokens'"

Some models reject chronikl's default request parameters: Anthropic's Claude 5.5 models refuse `temperature`, and OpenAI's GPT-5 family on Chat Completions wants `max_completion_tokens` instead of `max_tokens` (its reasoning models also refuse `temperature`). chronikl handles this itself. You'll see a warning such as:

```
warn: model `claude-haiku-5-5` rejected `temperature`; retrying without `temperature` for the rest of this run
```

That call is retried once with the parameter changed, and later calls start that way. The audit log marks the call (`param_fallbacks`). To skip the first rejected call, set `CHRONIKL_TEMPERATURE=none` (or `[provider] temperature = "none"`).

## "warn: Tier 1 returned no verdict for N of M commit(s); asking again for those"

The model answered a batch with verdicts for only some of its commits; this happens now and then with models that run at their default temperature. chronikl sends the missing commits once more in their own batch. If it happens on every run, lower `[ladder] batch_size`.

## "LLM API error: HTTP 429 Too Many Requests"

You're being rate-limited by your provider. chronikl retries transient errors automatically (up to 5 times with exponential backoff capped at 60s). If you hit this consistently, batch_size in `[ladder]` is too large — try `batch_size = 25` to reduce per-call payload size, or use a different model.

## "could not parse LLM response as JSON"

The model returned malformed structured output. chronikl already tolerates markdown-fenced JSON, prose-prefixed JSON, and bare JSON arrays. If you're hitting this consistently:

- Try a more capable model (Sonnet/Opus over Haiku)
- For OpenAI-compatible / Ollama endpoints, ensure the runtime supports JSON-mode or constrained decoding

## "warn: prose pass failed; falling back to deterministic render: …"

The prose pass errored (rate limit, timeout, model issue). The deterministic Tier 0 → Markdown render kicks in so you still get notes — they just won't have the voice applied. For user-facing notes (`--voice simple`, or `user_facing = true`) the fallback is one line such as "Bug fixes and stability improvements." instead of raw commit subjects. Re-run later, or use `chronikl debug prompts` to inspect what was about to be sent.

## How do I tell model-written notes from the fallback in CI?

Read `notes_source.kind` from the audit log (`--audit-log`):

| `kind` | Meaning |
|---|---|
| `model` | The prose pass wrote the notes. |
| `fallback` | The model was wanted but failed or no provider was usable (`reason` says which); the deterministic renderer or the fallback line wrote the notes. |
| `deterministic` | `--no-llm`, or no commits for the model to describe. |
| `nothing_user_facing` | User-facing notes with nothing user-facing in the range; the fallback line for `fallback` (`fixes`, `performance` or `other`) was written. |

Or run with `--strict`: then a `fallback` run exits 1 and writes nothing.

## "strict mode: the notes would have fallen back to the deterministic renderer (…); nothing was written"

`--strict` (or `CHRONIKL_STRICT=true`, `[output] strict = true`) is on and the LLM pipeline couldn't write the notes: the reason in parentheses says why (no provider or key, an unusable provider, a failed prose pass). stdout and `--output` stay empty; `--audit-log` is still written. Fix the provider, or drop `--strict` to publish the deterministic notes.

## The notes say only "Small improvements behind the scenes."

That's the fallback line of user-facing notes: nothing in the range is something a user would notice (every commit was in an excluded section, or the model said so). The line depends on what the commits were (fixes, performance, anything else) and can be changed with `fallback_line`, `fallback_fixes`, `fallback_performance` and `fallback_other` (see [Configuration](05-Configuration#user-facing-notes)).

## "invalid frontmatter in voice …"

The `---` block at the top of a voice file has a key chronikl doesn't know, a value of the wrong type, or a line that isn't `key: value`. The keys are the `[render]` keys (see [Voice](06-Voice#frontmatter)). `chronikl validate <FILE>` shows the error.

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

## Related Pages

- [CLI Reference](08-CLI-Reference) — every flag
- [Configuration](05-Configuration) — env vars + TOML
