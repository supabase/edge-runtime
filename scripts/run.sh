#!/usr/bin/env bash

 cargo build && RUST_BACKTRACE=full ./target/debug/edge-runtime "$@" start --main-service ./examples/main --event-manager ./examples/event-manager
