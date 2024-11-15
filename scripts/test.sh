#!/usr/bin/env bash

if [[ -n "$RUST_LOG" ]]; then
  cargo test --features base/tracing --test integration_tests -- "$@"
else
  cargo test "$@"
fi
