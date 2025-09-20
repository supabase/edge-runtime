#!/usr/bin/env bash
set -e

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
while [ -h "$SCRIPT_SOURCE" ]; do
  DIR="$(cd -P "$(dirname "$SCRIPT_SOURCE")" >/dev/null 2>&1 && pwd)"
  SCRIPT_SOURCE="$(readlink "$SCRIPT_SOURCE")"
  [[ "$SCRIPT_SOURCE" != /* ]] && SCRIPT_SOURCE="$DIR/$SCRIPT_SOURCE"
done
SCRIPTPATH="$(cd -P "$(dirname "$SCRIPT_SOURCE")" >/dev/null 2>&1 && pwd)"

export $(grep -v '^#' $SCRIPTPATH/../.env | xargs)

# --features cli/tracing
cargo build --features cli/tracing && \
RUST_BACKTRACE=full ./target/debug/edge-runtime "$@" start \
    --main-service "$SCRIPTPATH/../examples/main" \
    --event-worker "$SCRIPTPATH/../examples/event-manager"
