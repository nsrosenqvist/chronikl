# Voice

The "voice" is the system prompt that drives chronikl's prose pass. By default chronikl uses a bundled voice — concise, factual, professional. To change tone, supply your own Markdown file.

---

## Default behaviour

With no voice configured, chronikl uses an embedded default that produces:

- Concise, factual entries
- One line per item, leading with a verb
- PR numbers in parentheses where present
- No author or date noise

Plenty of releases need nothing more than this.

## Custom voice file

A voice file is plain Markdown — the whole file is used as the system prompt. No frontmatter required.

`my-voice.md`:

```markdown
You are the chief storyteller for Acme Corp's release notes.

Style:
- Warm but professional. Avoid corporate jargon.
- Lead with what users gain, not what engineers built.
- Use Oxford commas. Use plain English.
- When mentioning a feature, hint at the use case.

Output GitHub-flavored Markdown. Group entries under section headers.
Skip empty sections.
```

Use it via:

```bash
chronikl --voice ./my-voice.md
```

Or persist it via TOML:

```toml
[voice]
path = "release-voice.md"
```

Validate the file is readable + non-empty:

```bash
chronikl validate ./my-voice.md
```

## One-off addendum (`--prompt`)

For an ad-hoc tweak that you don't want to commit to the voice file:

```bash
chronikl --prompt "Mention Q4 launch in the opening line."
```

This text is appended *after* the voice body and any TOML `voice.extra_instructions`, so it always wins on conflicts.

## Auto-applied addenda

chronikl detects the release context and auto-prepends short instructions to the system prompt before your user-supplied content:

- **Prerelease detection** (`v1.0.0-rc.1`, `v0.5.0-alpha`, etc.) → "treat entries as in-progress / experimental, lead with caveats"
- **Version bump kind** (Major/Minor/Patch/Initial, semver or CalVer) → kind-specific framing (e.g. "lead with breaking changes" for semver Major; "lead with bug fixes" for Patch)

Order: `voice body → bump addendum → prerelease addendum → TOML extras → --prompt`. Your `--prompt` always wins last.

To inspect what gets sent without calling the LLM:

```bash
chronikl debug prompts --from v0.1.0 --to v0.2.0
```

This dumps the assembled system prompt + user prompt for every tier and the prose pass.

## Related Pages

- [Configuration](04-Configuration) — `[voice]` TOML section
- [CLI Reference](07-CLI-Reference) — `--voice`, `--prompt`, `validate`, `debug prompts`
