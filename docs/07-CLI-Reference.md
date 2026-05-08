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

### Voice flags

| Flag | Effect |
|---|---|
| `--voice <FILE>` | Path to a voice markdown file. Overrides `voice.path` in TOML. |
| `--prompt <TEXT>` | One-shot system-prompt addendum. Applied last; always wins. |

### LLM behaviour flags

| Flag | Effect |
|---|---|
| `--no-llm` | Skip Tier 1+ and the prose pass. Tier 0 deterministic classification + Markdown render still run. |
| `--agent` | Enable Tier 3 agentic fallback (read-only repo tools for ambiguous commits). |
| `--no-cache` | Skip the on-disk classification cache. |
| `--no-pr-enrichment` | Skip PR fetching even when a GitHub remote is detected. Also `CHRONIKL_NO_PR_ENRICHMENT=true`. |
| `--no-telemetry` | Disable the heartbeat for this run. |

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

### `chronikl license {activate | status | deactivate}`

Manage the chronikl license key.

```bash
chronikl license activate <KEY>     # verify and write to ~/.config/chronikl/license.key
chronikl license activate           # read from stdin
chronikl license status             # show currently active license
chronikl license deactivate         # remove the on-disk key
```

See [Licensing](08-Licensing).

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
| `chronikl debug prompts [range...] [--voice FILE] [--prompt TEXT]` | Dump every LLM prompt that *would* be sent (Tier 1, Tier 2, Tier 3, prose pass) without calling the LLM. Honours the configured voice + addenda. |

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
| `1` | Generic error — see stderr |

## Related Pages

- [Configuration](04-Configuration) — TOML alternatives
- [Voice](05-Voice) — voice file format
