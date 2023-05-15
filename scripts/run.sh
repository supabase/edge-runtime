#!/usr/bin/env bash

 cargo build && RUST_BACKTRACE=full RUST_LOG=debug ./target/debug/edge-runtime "$@" start --main-service ./examples/main
