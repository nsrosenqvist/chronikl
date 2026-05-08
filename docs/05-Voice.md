# Voice

The "voice" is the system prompt that drives chronikl's prose pass. chronikl ships **two bundled voice profiles** and accepts your own Markdown file when you want full control.

---

## Bundled profiles

| Profile | Use when… |
|---|---|
| `terse` *(default)* | You want compact one-line bullets, lead-with-verb, PR numbers in parens. The original chronikl style — fast to read, fits in a release email. |
| `prose` | You want richer multi-sentence explanations: bold lead phrase + em-dash + 2–3 sentence body for marquee items. Pairs well with `--rich-context`. |

`default` is accepted as an alias for `terse`, so existing scripts that pass `--voice default` keep working.

```bash
chronikl --voice terse        # explicit; same as the implicit default
chronikl --voice prose        # richer notes
chronikl --voice default      # alias → terse
```

Or persist via TOML:

```toml
[voice]
profile = "prose"
```

## Rich context

The prose pass normally sees only the commit subject, classification source, PR title, and PR labels per entry. With `--rich-context` (or `[voice].rich_context = true` in TOML), chronikl also embeds **truncated commit bodies (≤600 chars) and PR bodies (≤1000 chars)** under each entry. This is what lets the `prose` voice write rich, well-grounded explanations instead of paraphrasing the subject.

```bash
chronikl --voice prose --rich-context
```

```toml
[voice]
profile      = "prose"
rich_context = true
```

The terse voice also accepts richer context but won't make much use of it — the extra material is mostly wasted tokens unless you're using a voice that asks for explanations. Off by default for that reason.

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

`--voice` accepts either a bundled profile name (`terse`, `prose`, `default`) or a file path. If the value matches a profile name it loads the bundled content; otherwise it's treated as a path. Pass `./prose` (with the leading `./`) if you ever need to load a file literally named `prose`.

If both `[voice].path` and `[voice].profile` are set in TOML, `path` wins — bundled profiles are the easy default and the custom file is the more specific override.

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
