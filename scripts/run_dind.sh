#!/usr/bin/env bash

FEATURES=cli/tracing
RUST_BACKTRACE=full

PWD=$(pwd)
PROFILE=${1:-dind}
SCRIPT=$(readlink -f "$0")
SCRIPTPATH=$(dirname "$SCRIPT")

cd "$SCRIPTPATH"

docker build \
  -t edge_runtime \
  --env-file ../.env \
  --build-arg PROFILE=$PROFILE \
  --build-arg FEATURES=$FEATURES \
  "$SCRIPTPATH/.."

export $(grep -v '^#' ../.env | xargs)
docker run \
  --privileged \
  --rm \
  -it \
  -p $EDGE_RUNTIME_PORT:$EDGE_RUNTIME_PORT \
  -w /home/deno \
  -v "$SCRIPTPATH/../examples:/home/deno/examples" \
  -e EDGE_RUNTIME_PORT=$EDGE_RUNTIME_PORT \
  -e RUST_BACKTRACE=$RUST_BACKTRACE \
  -e RUST_LOG=$RUST_LOG \
  edge_runtime:latest \
  start \
  -p $EDGE_RUNTIME_PORT \
  --main-service ./examples/main \
  --event-worker ./examples/event-manager \
  --static "./examples/**/*.bin"

cd $PWD
