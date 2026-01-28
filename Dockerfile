# Builder Stage
FROM rust:1.93-slim-bookworm as builder

WORKDIR /app
COPY . .

# Install build dependencies
RUN apt-get update && apt-get install -y pkg-config libssl-dev protobuf-compiler

# Build API
RUN cargo build --release -p api

# Build Worker
RUN cargo build --release -p worker

# Runtime Stage (API)
FROM debian:bookworm-slim as api
WORKDIR /app
RUN apt-get update && apt-get install -y libssl3 ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/api /app/api
CMD ["/app/api"]

# Runtime Stage (Worker)
FROM debian:bookworm-slim as worker
WORKDIR /app
# Install FFmpeg
RUN apt-get update && apt-get install -y libssl3 ca-certificates ffmpeg && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/worker /app/worker
CMD ["/app/worker"]
