FROM lukemathwalker/cargo-chef:latest-rust-1.93.0 AS chef
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
RUN rm -f /etc/apt/apt.conf.d/docker-clean && \
    echo 'Binary::apt::APT::Keep-Downloaded-Packages "true";' > /etc/apt/apt.conf.d/keep-cache
RUN --mount=type=cache,id=memegy-apt-cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,id=memegy-apt-lib,target=/var/lib/apt,sharing=locked \
    apt-get update && \
    apt-get install -y --no-install-recommends pkg-config libssl-dev protobuf-compiler clang mold && \
    rm -rf /var/lib/apt/lists/*

COPY --from=planner /app/recipe.json recipe.json
ENV RUSTFLAGS="-C link-arg=-fuse-ld=mold"
RUN --mount=type=cache,id=memegy-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=memegy-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=memegy-cargo-target,target=/app/target \
    cargo chef cook --release --recipe-path recipe.json

# ============================================
# API & WORKER BUILDER
# ============================================
FROM builder-base AS builder
ENV CARGO_BUILD_JOBS=2
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,id=memegy-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=memegy-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=memegy-cargo-target,target=/app/target \
    cargo build --release -p api -p worker && \
    mkdir -p /app/bin && \
    cp target/release/api /app/bin/api && \
    cp target/release/worker /app/bin/worker

# ============================================
# RUNTIME (Final shared API + Worker image)
# ============================================
FROM debian:bookworm-slim AS runtime
WORKDIR /app
RUN rm -f /etc/apt/apt.conf.d/docker-clean && \
    echo 'Binary::apt::APT::Keep-Downloaded-Packages "true";' > /etc/apt/apt.conf.d/keep-cache
RUN --mount=type=cache,id=memegy-apt-cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,id=memegy-apt-lib,target=/var/lib/apt,sharing=locked \
    apt-get update && \
    apt-get install -y --no-install-recommends libssl3 ca-certificates ffmpeg && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/bin/api /app/api
COPY --from=builder /app/bin/worker /app/worker
COPY openapi.yaml /app/openapi.yaml
COPY static /app/static
CMD ["/bin/sh"]

# Backward-compatible targets for non-production compose files.
FROM runtime AS api
CMD ["/app/api"]

FROM runtime AS worker
CMD ["/app/worker"]
