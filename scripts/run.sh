#!/usr/bin/env bash

export $(grep -v '^#' ../.env | xargs)

# --features cli/tracing
cargo build --features cli/tracing && \
RUST_BACKTRACE=full ./target/debug/edge-runtime "$@" start \
    --main-service ./examples/main \
    --event-worker ./examples/event-manager