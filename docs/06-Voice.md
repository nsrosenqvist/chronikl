# Voice

The "voice" is the system prompt that drives chronikl's prose pass. chronikl ships **three bundled voice profiles** and accepts your own Markdown file when you want full control.

---

## Bundled profiles

| Profile | Use when… |
|---|---|
| `terse` *(default)* | You want compact one-line bullets, lead-with-verb, PR numbers in parens. The original chronikl style — fast to read, fits in a release email. |
| `prose` | You want richer multi-sentence explanations: bold lead phrase + em-dash + 2–3 sentence body for marquee items. Pairs well with `--rich-context`. |
| `simple` | The notes are for the product's users: app-store "What's new" text, an update dialog, an in-app changelog. Short plain-language bullets about what users can now do or will notice. Its frontmatter turns on the user-facing rendering: one flat plain-text list, no footer, internal sections dropped, at most 5 bullets and 500 characters, and a friendly fallback line when nothing is user-facing. |

`terse` and `prose` carry no rendering settings, so they keep chronikl's classic output: every section, headings, the compare-link footer.

```bash
chronikl --voice simple                    # app-store notes, ≤ 500 characters
chronikl --voice simple --max-chars 450    # flags override the voice's settings
```

`default` is accepted as an alias for `terse`, so existing scripts that pass `--voice default` keep working.

```bash
chronikl --voice terse        # explicit; same as the implicit default
chronikl --voice prose        # richer notes
chronikl --voice default      # alias → terse
chronikl --voice simple       # for the product's users
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

`--voice` accepts either a bundled profile name (`terse`, `prose`, `simple`, `default`) or a file path. If the value matches a profile name it loads the bundled content; otherwise it's treated as a path. Pass `./prose` (with the leading `./`) if you ever need to load a file literally named `prose`.

If both `[voice].path` and `[voice].profile` are set in TOML, `path` wins — bundled profiles are the easy default and the custom file is the more specific override.

Validate the file is readable, its frontmatter valid and its body non-empty:

```bash
chronikl validate ./my-voice.md
```

## Frontmatter

A voice can carry its rendering settings in a frontmatter block at the top of the file, between `---` lines. The keys are the `[render]` keys from [Configuration](05-Configuration#rendering-render), written as `key: value`:

```markdown
---
headings: false
footer: false
format: plain
user_facing: true
exclude_sections: [refactor, docs, tests, build, ci, chore]
max_bullets: 5
max_chars: 450                 # Google Play allows 500
fallback_other: Small improvements behind the scenes.
---
You write the "What's new" text that testers read in Google Play…
```

- Values are booleans (`true`/`false`), whole numbers, strings (quotes optional) or lists (`[a, b]`, or one `- item` per line under the key). `#` starts a comment.
- Unknown keys are an error (`chronikl validate` reports them), so a typo doesn't silently do nothing.
- The frontmatter isn't sent to the model; the body after it is the system prompt.
- `.chronikl.toml` `[render]`, environment variables and flags override the frontmatter key by key.

This is how the bundled `simple` voice works: its file is a frontmatter block plus a body. A custom voice file can opt into the same behavior the same way.

## One-off addendum (`--prompt`)

For an ad-hoc tweak that you don't want to commit to the voice file:

```bash
chronikl --prompt "Mention Q4 launch in the opening line."
```

This text is appended *after* the voice body and any TOML `voice.extra_instructions`, so it always wins on conflicts.

## Auto-applied addenda

chronikl detects the release context and the run's settings, and appends short instructions to the system prompt before your user-supplied content:

- **Prerelease detection** (`v1.0.0-rc.1`, `v0.5.0-alpha`, etc.) → "treat entries as in-progress / experimental, lead with caveats"
- **Version bump kind** (Major/Minor/Patch/Initial, semver or CalVer) → kind-specific framing (e.g. "lead with breaking changes" for semver Major; "lead with bug fixes" for Patch)
- **User-facing notes** (`user_facing = true`, as in `simple`) → keep only what users can see or do, in plain words; leave out internal and staff-only items even when they're classified as features or fixes.
- **Output format** — whenever the render settings ask for something other than sectioned Markdown (headings off, bullet or character limits, plain text, footer off, user-facing notes) → an "Output format" section stating those rules, which take precedence over the voice's own document-shape guidance. For user-facing notes it also asks the model to answer `NO_USER_FACING_CHANGES` when nothing qualifies; chronikl then writes the fallback line.

The prerelease and bump framings describe a sectioned developer document, so they're added only with headings on and user-facing notes off.

Order: `voice body → bump addendum → prerelease addendum → user-facing notes → output format → TOML extras → --prompt`. Your `--prompt` always wins last.

The prose pass's user message asks for the same shape as the "Output format" section, so the two never disagree. The bundled voices describe their sectioned layout as the default and switch to a flat list when asked. A custom voice that insists on a shape of its own still loses to the format rules, and chronikl enforces the shape on the model's answer anyway: headings dropped, Markdown stripped, bullets cut at whole bullets to fit the limits.

To inspect what gets sent without calling the LLM:

```bash
chronikl debug prompts --from v0.1.0 --to v0.2.0
```

This dumps the assembled system prompt + user prompt for every tier and the prose pass.

## Related Pages

- [Configuration](05-Configuration) — `[voice]` TOML section
- [CLI Reference](08-CLI-Reference) — `--voice`, `--prompt`, `validate`, `debug prompts`
