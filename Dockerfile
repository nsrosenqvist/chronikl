# Multi-stage Dockerfile for chronikl
# Produces a minimal runtime image with `git` available so the binary
# can read commit history mounted at /repo.

# ── Stage 1: build ──────────────────────────────────────────────────
FROM rust:1.89-bookworm AS builder

WORKDIR /build

# Cache dependencies by building a dummy project first.
COPY Cargo.toml Cargo.lock* build.rs ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && \
    cargo build --release 2>/dev/null || true && \
    rm -rf src

# Copy source and build for real.
COPY src/ src/
COPY tools/ tools/

RUN cargo build --release --bin chronikl && \
    strip target/release/chronikl

# ── Stage 2: runtime ────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        git \
        ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/chronikl /usr/local/bin/chronikl

# Default working directory — mount your repo here.
WORKDIR /repo

ENTRYPOINT ["chronikl"]
CMD ["--help"]
