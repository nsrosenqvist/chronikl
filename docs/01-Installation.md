# Installation

chronikl ships as a single static binary for Linux and macOS (x86_64 and aarch64), as a Docker image, and as a GitHub Action.

---

## One-shot installer

```bash
curl -sSfL https://raw.githubusercontent.com/nsrosenqvist/chronikl/main/install.sh | sh
```

Detects platform, downloads the latest release tarball, verifies SHA256, and installs to `/usr/local/bin/chronikl`.

Options:

```bash
# Specific version:
curl -sSfL .../install.sh | sh -s -- --version v0.1.0

# Custom install dir:
curl -sSfL .../install.sh | sh -s -- --dir ~/.local/bin

# Or via env vars:
CHRONIKL_VERSION=v0.1.0 CHRONIKL_INSTALL_DIR=~/.local/bin .../install.sh
```

## Manual download

Grab the right tarball from the [releases page](https://github.com/nsrosenqvist/chronikl/releases) and extract:

```bash
# Linux x86_64
curl -sSfL https://github.com/nsrosenqvist/chronikl/releases/latest/download/chronikl-x86_64-unknown-linux-gnu.tar.gz \
  | sudo tar xz -C /usr/local/bin

# Linux aarch64
curl -sSfL https://github.com/nsrosenqvist/chronikl/releases/latest/download/chronikl-aarch64-unknown-linux-gnu.tar.gz \
  | sudo tar xz -C /usr/local/bin

# macOS x86_64
curl -sSfL https://github.com/nsrosenqvist/chronikl/releases/latest/download/chronikl-x86_64-apple-darwin.tar.gz \
  | sudo tar xz -C /usr/local/bin

# macOS aarch64
curl -sSfL https://github.com/nsrosenqvist/chronikl/releases/latest/download/chronikl-aarch64-apple-darwin.tar.gz \
  | sudo tar xz -C /usr/local/bin
```

## Homebrew (macOS / Linux)

```bash
brew install nsrosenqvist/chronikl/chronikl
```

## Docker

```bash
docker run --rm -v "$PWD:/repo" ghcr.io/nsrosenqvist/chronikl --from v0.1.0 --to HEAD
```

The image runs `chronikl` as its entrypoint with `WORKDIR /repo`. Mount your repo at `/repo` and pass any chronikl args. `git` and CA certificates are included.

## crates.io

```bash
cargo install chronikl
```

Builds from source. Requires Rust 1.85+.

## Self-update

If chronikl is already installed:

```bash
chronikl update
chronikl update --force   # re-install even when up-to-date
```

Downloads the latest release, verifies SHA256, and atomically replaces the running binary.

## Verify the install

```bash
chronikl --version
```

## Related Pages

- [Quick Start](02-Quick-Start) — generate your first release notes
- [LLM Providers](03-Providers) — provider setup
