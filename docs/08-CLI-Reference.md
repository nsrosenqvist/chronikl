# CLI Reference

```
chronikl [OPTIONS] [COMMAND]
```

With no subcommand, `chronikl` runs the default *generate notes* flow.

---

## Default invocation (generate notes)

```bash
chronikl                                    # auto: previous semver tag → HEAD
chronikl --from <REF> --to <REF>            # explicit range
chronikl --since-last-tag                   # latest tag → HEAD, even when HEAD is tagged
```

### Range flags

| Flag | Effect |
|---|---|
| `--from <REF>` | Lower-bound ref (exclusive). |
| `--to <REF>` | Upper-bound ref. Defaults to `HEAD`. |
| `--since-last-tag` | Use the latest semver tag as `from` even when HEAD is itself tagged. Mutually exclusive with `--from`/`--to`. |
| `--repo <PATH>` | Repository path (default: cwd). |

### Output flags

| Flag | Effect |
|---|---|
| `-o, --output <FILE>` | Write Markdown to a file instead of stdout. |
| `--audit-log <FILE>` | Write a JSON audit log of every LLM call. Also `CHRONIKL_AUDIT_LOG=...`. |
| `--strict` | Exit non-zero and write no notes when they would fall back to the deterministic renderer (provider missing or unusable, prose pass failed). A user-facing range with nothing user-facing in it isn't a failure. Ignored with `--no-llm`. Also `CHRONIKL_STRICT=true`, `[output] strict = true`. |

### Rendering flags

Each flag has a `[render]` key (also usable in a voice's frontmatter) and a `CHRONIKL_*` variable; see [Configuration](05-Configuration#rendering-render). Flags win over all of them.

| Flag | Effect |
|---|---|
| `--headings[=true\|false]` | Section headings on (default) or off. Off writes one flat bullet list. |
| `--include-sections <IDS>` | Keep only these sections (comma-separated, e.g. `features,fixes`). |
| `--exclude-sections <IDS>` | Drop these sections before the prose pass (e.g. `tests,docs,ci`). |
| `--max-bullets <N>` | Keep at most N bullets, cut at whole bullets in output order. |
| `--max-chars <N>` | Keep the whole output within N characters, cut at whole bullets. |
| `--footer[=true\|false]` | The `**Full Changelog**` compare link on (default) or off. |
| `--format <markdown\|plain>` | `plain` writes text without Markdown (`- ` bullets, no emphasis, links or `#`). |
| `--user-facing[=true\|false]` | Notes for the product's users: only what they'd notice, and a fallback line when nothing is user-facing. The `simple` voice turns it on. |
| `--fallback-line <TEXT>` | "Nothing user-facing" line for any range. |
| `--fallback-fixes <TEXT>`, `--fallback-performance <TEXT>`, `--fallback-other <TEXT>` | … for a range with bug fixes, with performance work, or anything else. Win over `--fallback-line`. |

Section ids: `breaking`, `features`, `fixes` (or `bug-fixes`), `performance` (`perf`), `refactor`, `docs` (`documentation`), `tests`, `build`, `ci`, `chore`, `other`.

### Voice flags

| Flag | Effect |
|---|---|
| `--voice <NAME_OR_FILE>` | Bundled profile name (`terse`, `prose`, `simple`, `default`) or path to a custom voice markdown file. Bundled-name match wins over treating it as a path. Overrides `voice.path` / `voice.profile` in TOML. |
| `--rich-context` | Embed truncated commit bodies (≤600 chars) and PR bodies (≤1000 chars) in the prose-pass user prompt. Off by default; pairs well with `--voice prose`. Also `[voice].rich_context = true` in TOML. |
| `--prompt <TEXT>` | One-shot system-prompt addendum. Applied last; always wins. |

### LLM behaviour flags

| Flag | Effect |
|---|---|
| `--no-llm` | Skip Tier 1+ and the prose pass. Tier 0 deterministic classification + Markdown render still run. |
| `--agent` | Enable Tier 3 agentic fallback (read-only repo tools for ambiguous commits). |
| `--no-cache` | Skip the on-disk classification cache. |
| `--no-pr-enrichment` | Skip PR fetching even when a GitHub remote is detected. Also `CHRONIKL_NO_PR_ENRICHMENT=true`. |
| `--temperature <N\|none>` | Sampling temperature for every LLM call (default `0`). `none` sends no temperature. A model that rejects it (Claude 5.5, OpenAI reasoning models) gets the call retried once without it, and the rest of the run leaves it out. Also `CHRONIKL_TEMPERATURE`, `[provider] temperature`. |

## Subcommands

### `chronikl version`

Print version, build commit, build date, target triple.

### `chronikl validate <FILE>`

Validate a voice markdown file. Checks readability and that the body is non-empty.

```bash
chronikl validate ./my-voice.md
```

### `chronikl cache {path | stats | clear}`

Manage the on-disk classification cache.

```bash
chronikl cache path     # print the cache root
chronikl cache stats    # entries + total bytes
chronikl cache clear    # remove every cached classification
```

### `chronikl update [--force]`

Self-update from the latest GitHub release. Verifies SHA256 before swapping the binary.

```bash
chronikl update
chronikl update --force     # re-install even when up-to-date
```

### `chronikl debug <SUBCOMMAND>`

Diagnostic helpers. Output shapes are subject to change.

| Subcommand | What it does |
|---|---|
| `chronikl debug commits [range...]` | Dump parsed commits as JSON. |
| `chronikl debug merge-style [range...]` | Print the detected merge style (`squash`, `merge`, `rebase`, `mixed`). |
| `chronikl debug config` | Print the resolved configuration. |
| `chronikl debug classify [range...]` | Run Tier 0 deterministic classification, print JSON. |
| `chronikl debug prompts [range...] [--voice NAME_OR_FILE] [--prompt TEXT] [--rich-context] [rendering flags]` | Dump every LLM prompt that *would* be sent (Tier 1, Tier 2, Tier 3, prose pass) without calling the LLM. Honors the configured voice + addenda and the rendering flags, so the dumped prose prompt matches what `generate` would actually send. |

## Provider / model overrides

These are typically set via env or `.chronikl.toml`, but can be overridden inline:

```bash
chronikl --provider anthropic \
         --model claude-haiku-4-5 \
         --api-key sk-ant-... \
         --base-url ...    # only for openai-compatible / azure / ollama
```

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Error, including a `--strict` run whose notes would have fallen back — see stderr |

## Related Pages

- [Configuration](05-Configuration) — TOML alternatives
- [Voice](06-Voice) — voice file format
