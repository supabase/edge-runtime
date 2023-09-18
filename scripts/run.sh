#!/usr/bin/env bash

GIT_V_TAG=0.1.1 cargo build && RUST_BACKTRACE=full ./target/debug/edge-runtime "$@" start --main-service ./examples/main --event-worker ./examples/event-manager
