#!/usr/bin/env bash

GIT_V_TAG=0.1.1 cargo build && RUST_BACKTRACE=full ./target/debug/edge-runtime "$@" start --main-service ./examples/main --event-worker ./examples/event-manager
#GIT_V_TAG=0.1.1 cargo build && RUST_BACKTRACE=full ./target/debug/edge-runtime "$@" start --main-service ./main.eszip --main-entrypoint file:///Users/lakshanperera/workspace/edge-runtime/examples/main/index.ts --event-worker ./event-manager.eszip --events-entrypoint file:///Users/lakshanperera/workspace/edge-runtime/examples/event-manager/index.ts
