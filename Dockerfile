# syntax=docker/dockerfile:1.4
FROM rust:1.67.0-bullseye as builder
WORKDIR /usr/src/edge-runtime
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo install cargo-strip
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/target \
    cargo build --release --target-dir /usr/target && \
    cargo strip

FROM debian:bullseye-slim
RUN apt-get update && apt-get install -y libssl-dev && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/target/release/edge-runtime /usr/local/bin/edge-runtime
ENTRYPOINT ["edge-runtime"]
