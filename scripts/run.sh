#!/usr/bin/env bash

 cargo build && RUST_LOG=debug RUST_BACKTRACE=full ./target/debug/edge-runtime "$@" start --main-service ./examples/main
