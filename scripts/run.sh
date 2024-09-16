#!/usr/bin/env bash

# --features cli/tracing
GIT_V_TAG=0.1.1 cargo build --features cli/tracing && \
AI_INFERENCE_API_HOST=http://localhost:11434 \
EDGE_RUNTIME_PORT=9998 RUST_BACKTRACE=full ./target/debug/edge-runtime "$@" start \
    --main-service ./examples/main \
    --event-worker ./examples/event-manager
