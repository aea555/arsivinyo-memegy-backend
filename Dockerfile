# ============================================
# CHEF STAGE (Shared - for cargo-chef)
# ============================================
FROM rust:1.93-slim-bookworm AS chef
RUN cargo install cargo-chef
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
# Install build dependencies (including mold) early
RUN apt-get update && apt-get install -y pkg-config libssl-dev protobuf-compiler clang mold && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
# Set RUSTFLAGS here so dependencies are cooked with mold found
ENV RUSTFLAGS="-C link-arg=-fuse-ld=mold"
RUN cargo chef cook --release --recipe-path recipe.json

# ============================================
# API & WORKER BUILDER
# ============================================
# Build application
FROM builder-base AS builder
# RUSTFLAGS and tools are inherited from builder-base
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# Build both binaries. Cargo will share compilation of 'shared' crate.
RUN cargo build --release -p api -p worker

# ============================================
# API RUNTIME (Final API image)
# ============================================
FROM debian:bookworm-slim AS api
WORKDIR /app
RUN apt-get update && apt-get install -y libssl3 ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/api /app/api
COPY openapi.yaml /app/openapi.yaml
COPY static /app/static
CMD ["/app/api"]

# ============================================
# WORKER RUNTIME (Final Worker image)
# ============================================
FROM debian:bookworm-slim AS worker
WORKDIR /app
RUN apt-get update && apt-get install -y libssl3 ca-certificates ffmpeg && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/worker /app/worker
CMD ["/app/worker"]
