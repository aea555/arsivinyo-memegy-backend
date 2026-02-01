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
RUN apt-get update && apt-get install -y pkg-config libssl-dev protobuf-compiler && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# ============================================
# API & WORKER BUILDER (Builds BOTH binaries in one go)
# ============================================
FROM builder-base AS builder
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# Build both binaries. Cargo will share compilation of 'shared' crate.
RUN cargo build --release -p api -p worker -vv

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
