#!/usr/bin/env bash

cargo clippy --workspace --all-targets --all-features -- -D warnings
