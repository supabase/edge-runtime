#!/usr/bin/env bash

GIT_V_TAG=0.1.1 cargo build && ORT_DYLIB_PATH=/Users/lakshanperera/workspace/edge-runtime/libonnxruntime.so.1.17.0 SB_AI_MODELS_DIR=/Users/lakshanperera/workspace/edge-runtime/models EDGE_RUNTIME_PORT=9000 RUST_BACKTRACE=full ./target/debug/edge-runtime "$@" start --main-service ./examples/main --event-worker ./examples/event-manager
