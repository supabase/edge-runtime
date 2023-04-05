#!/usr/bin/env bash

cargo build && ./target/debug/edge-runtime "$@"
