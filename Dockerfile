# ============================================
# CHEF STAGE (Shared - for cargo-chef)
# ============================================
FROM rust:1.93-slim-bookworm AS chef
# Install cargo-chef (cached via registry mount)
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo install cargo-chef
WORKDIR /app

# ============================================
# PLANNER STAGE (Shared - analyzes dependencies)
# ============================================
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ============================================
# BUILDER BASE (Shared - installs build deps & cooks dependencies)
# ============================================
FROM chef AS builder-base
# Install build dependencies with cache
RUN rm -f /etc/apt/apt.conf.d/docker-clean; echo 'Binary::apt::APT::Keep-Downloaded-Packages "true";' > /etc/apt/apt.conf.d/keep-cache
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update && apt-get install -y pkg-config libssl-dev protobuf-compiler clang mold

COPY --from=planner /app/recipe.json recipe.json
# Set RUSTFLAGS here so dependencies are cooked with mold found
ENV RUSTFLAGS="-C link-arg=-fuse-ld=mold"
# Cook dependencies with cache mounts
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo chef cook --release --recipe-path recipe.json

# ============================================
# API & WORKER BUILDER
# ============================================
# Build application
FROM builder-base AS builder
# Limit concurrency to avoid OOM on small CI runners
ENV CARGO_BUILD_JOBS=2
# RUSTFLAGS and tools are inherited from builder-base
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# Build both binaries with cache mounts.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release -p api -p worker && \
    mkdir -p /app/bin && \
    cp target/release/api /app/bin/api && \
    cp target/release/worker /app/bin/worker

# ============================================
# API RUNTIME (Final API image)
# ============================================
FROM debian:bookworm-slim AS api
WORKDIR /app
RUN rm -f /etc/apt/apt.conf.d/docker-clean; echo 'Binary::apt::APT::Keep-Downloaded-Packages "true";' > /etc/apt/apt.conf.d/keep-cache
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update && apt-get install -y libssl3 ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/bin/api /app/api
COPY openapi.yaml /app/openapi.yaml
COPY static /app/static
CMD ["/app/api"]

# ============================================
# WORKER RUNTIME (Final Worker image)
# ============================================
FROM debian:bookworm-slim AS worker
WORKDIR /app
RUN rm -f /etc/apt/apt.conf.d/docker-clean; echo 'Binary::apt::APT::Keep-Downloaded-Packages "true";' > /etc/apt/apt.conf.d/keep-cache
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update && apt-get install -y libssl3 ca-certificates ffmpeg && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/bin/worker /app/worker
CMD ["/app/worker"]
