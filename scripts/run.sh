#!/usr/bin/env bash

GIT_V_TAG=0.1.1 cargo build && EDGE_RUNTIME_PORT=9998 RUST_BACKTRACE=full ./target/debug/edge-runtime "$@" start --main-service ./examples/main --event-worker ./examples/event-manager --static "./examples/**/*.json"
