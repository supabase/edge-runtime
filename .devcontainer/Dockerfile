FROM mcr.microsoft.com/devcontainers/rust:dev-1-bookworm

RUN apt-get update && apt-get install -y build-essential cmake libclang-dev lldb \
    nodejs npm hyperfine

# https://github.com/tokio-rs/console
RUN cargo install tokio-console