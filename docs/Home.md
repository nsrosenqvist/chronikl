# chronikl Documentation

AI-powered release notes for your team. Bring your own model, bring your own API key. Designed to run inside your release workflow.

> **Output is advisory.** chronikl's prose pass is LLM-generated and can mischaracterise breaking changes, omit context, or invent motivation that isn't in the commits. Review every release note before tagging the release. The optional `--audit-log` flag records every LLM call (model, tokens, prompt + response hashes) so you can reconstruct *why* chronikl said something months later — see [CLI Reference](08-CLI-Reference).

---

## Getting Started

New to chronikl? Start here:

1. **[Installation](01-Installation)** — download the binary, install from source, or pull the Docker image.
2. **[Quick Start](02-Quick-Start)** — generate your first release notes in under two minutes.
3. **[LLM Providers](03-Providers)** — connect Anthropic, OpenAI, Gemini, or any compatible API.
4. **[GitHub Models](04-GitHub-Models)** — zero-secret setup in GitHub Actions via `GITHUB_TOKEN`.

## Using chronikl

- **[Configuration](05-Configuration)** — `.chronikl.toml`, environment variables, and CLI flags.
- **[Voice](06-Voice)** — control tone with a custom Markdown file or one-off `--prompt`.
- **[CI/CD Integration](07-CI-Integration)** — GitHub Actions, GitLab CI, etc.

## Reference

- **[CLI Reference](08-CLI-Reference)** — every command and flag.
- **[Licensing](09-Licensing)** — free tier, commercial activation.
- **[Troubleshooting](10-Troubleshooting)** — common issues and solutions.

---

[Website](https://chronikl.dev) · [GitHub](https://github.com/nsrosenqvist/chronikl) · [Report an Issue](https://github.com/nsrosenqvist/chronikl/issues)
