FROM rust:1.67.0-bullseye as builder
WORKDIR /usr/src/edge-runtime
COPY . .
RUN cargo clean && \
    cargo build -vv --release --target-dir /usr/target

FROM debian:bullseye-slim
RUN apt-get update && apt-get install -y libssl-dev && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/target/release/edge-runtime /usr/local/bin/edge-runtime
ENTRYPOINT ["edge-runtime"]
