# syntax=docker/dockerfile:1.6

# =====================================================================
# Stage 1: Builder — compiles the Rust binary
# =====================================================================
FROM rust:1.88-slim AS builder

WORKDIR /app

# System packages needed to link Rust binaries (openssl, etc.)
RUN apt-get update && \
    apt-get install -y --no-install-recommends pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# ---------------------------------------------------------------------
# Layer A: Copy only the manifests (Cargo.toml files).
# If these don't change, Docker reuses this layer on subsequent builds.
# ---------------------------------------------------------------------
COPY Cargo.toml Cargo.lock ./
COPY crates/api/Cargo.toml              crates/api/
COPY crates/core/Cargo.toml             crates/core/
COPY crates/session-store/Cargo.toml    crates/session-store/
COPY crates/circuit-breaker/Cargo.toml  crates/circuit-breaker/
COPY crates/priority-queue/Cargo.toml   crates/priority-queue/
COPY crates/telephony/Cargo.toml        crates/telephony/

# ---------------------------------------------------------------------
# Layer B: Create stub source files so cargo can resolve the graph.
# The actual code comes later — this just lets cargo see the crate layout.
# ---------------------------------------------------------------------
RUN mkdir -p crates/api/src && echo "fn main(){}" > crates/api/src/main.rs && \
    for c in core session-store circuit-breaker priority-queue telephony; do \
      mkdir -p crates/$c/src && echo "" > crates/$c/src/lib.rs; \
    done

# ---------------------------------------------------------------------
# Layer C: Pre-build dependencies.
# This layer is CACHED — cache mounts keep the downloaded crates and
# compiled .rlib files between builds.
# ---------------------------------------------------------------------
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release --bin sayangcare || true

# ---------------------------------------------------------------------
# Layer D: Copy the real source code and build the actual binary.
# Only YOUR crates recompile here — dependencies are already cached.
# ---------------------------------------------------------------------
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
        cargo clean --release \
            -p sayangcare-api \
            -p sayangcare-core \
            -p sayangcare-session-store \
            -p sayangcare-circuit-breaker \
            -p sayangcare-priority-queue \
            -p sayangcare-telephony && \
    cargo build --release --bin sayangcare && \
    cp target/release/sayangcare /app/sayangcare

# =====================================================================
# Stage 2: Runtime — small final image (no build tools)
# =====================================================================
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/sayangcare /usr/local/bin/sayangcare
COPY --from=builder /app/config /app/config
COPY --from=builder /app/migrations /app/migrations

EXPOSE 8080

CMD ["sayangcare"]
