# CI/CD Integration

chronikl is designed to run inside a release workflow. The release-tag push triggers it, chronikl writes notes to a file, and a downstream step publishes them to the GitHub Release.

---

## GitHub Actions (recommended)

Use the published action:

```yaml
# .github/workflows/release.yml
name: Release
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
        with:
          fetch-depth: 0    # required so chronikl can see all tags

      - name: Generate release notes
        uses: nsrosenqvist/chronikl@v0
        with:
          output: release_notes.md
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          CHRONIKL_PROVIDER: anthropic
          CHRONIKL_MODEL: claude-sonnet-4-6
          CHRONIKL_API_KEY: ${{ secrets.CHRONIKL_API_KEY }}

      - name: Publish release
        uses: softprops/action-gh-release@v2
        with:
          body_path: release_notes.md
```

`fetch-depth: 0` is important — without all tags chronikl can't resolve `--auto` ranges or detect the previous tag.

`GITHUB_TOKEN` is auto-injected by Actions and gives chronikl PR-read access to the current repo.

### Action inputs

| Input | Default | Notes |
|---|---|---|
| `version` | `latest` | chronikl release tag |
| `args` | (empty) | If non-empty, runs `chronikl <args>` and ignores the other inputs |
| `from` | (auto) | Lower-bound ref |
| `to` | (auto, usually `HEAD`) | Upper-bound ref |
| `since_last_tag` | `false` | Use latest semver tag as `from` even when HEAD is itself tagged |
| `voice` | (none) | Bundled profile name (`terse`, `prose`) or path to a voice markdown file |
| `rich_context` | `false` | Embed truncated commit/PR bodies in the prose-pass user prompt — pairs well with `voice: prose` |
| `prompt` | (none) | One-off system-prompt addendum |
| `output` | (stdout) | Write notes to this path |
| `agent` | `false` | Enable Tier 3 agentic fallback |
| `audit_log` | (none) | Write a JSON audit log to this path |

### Pass arbitrary args

If the inputs above don't cover what you need, drop down to raw args:

```yaml
      - uses: nsrosenqvist/chronikl@v0
        with:
          args: --from v1.0.0 --to v2.0.0 --voice docs/voice.md --output notes.md
        env:
          CHRONIKL_API_KEY: ${{ secrets.CHRONIKL_API_KEY }}
```

## Other CIs

For non-GitHub CIs, install the binary and run it directly:

```bash
curl -sSfL https://raw.githubusercontent.com/nsrosenqvist/chronikl/main/install.sh | sh
chronikl --output release_notes.md
```

PR enrichment is GitHub-only. On other forges chronikl logs `PR enrichment skipped (no GitHub remote detected)` and continues without it.

## Self-hosted runners + private models

The action only ships the binary; the LLM call itself happens from the runner. If you need to talk to a private LLM endpoint:

```yaml
        env:
          CHRONIKL_PROVIDER: openai-compatible
          CHRONIKL_BASE_URL: https://my-llm.internal/v1
          CHRONIKL_MODEL: my-model
          CHRONIKL_API_KEY: ${{ secrets.INTERNAL_LLM_KEY }}
```

## Audit log artifact

For provenance / debugging, capture a per-run audit log:

```yaml
      - uses: nsrosenqvist/chronikl@v0
        with:
          output: release_notes.md
          audit_log: release_audit.json
        env:
          CHRONIKL_API_KEY: ${{ secrets.CHRONIKL_API_KEY }}

      - uses: actions/upload-artifact@v6
        with:
          name: release-audit-${{ github.ref_name }}
          path: release_audit.json
```

The audit JSON includes every LLM call (model, tokens, prompt hash, response hash), per-tool-call records (when `--agent` is on), tier counts, the final classification, and the rendered Markdown — everything you need to answer "why did chronikl say that?" months later.

## Related Pages

- [LLM Providers](03-Providers) — provider/model selection
- [GitHub Models](04-GitHub-Models) — zero-secret CI setup using the auto-injected `GITHUB_TOKEN`
- [Configuration](05-Configuration) — `.chronikl.toml`
