#!/usr/bin/env bash

cargo build --release && ./target/debug/edge-runtime "$@"
