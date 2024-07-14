#!/usr/bin/env bash

GIT_V_TAG=0.1.1
EDGE_RUNTIME_PORT=9998
ONNXRUNTIME_VERSION=1.17.3
FEATURES=cli/tracing
RUST_BACKTRACE=full

SCRIPT=$(readlink -f "$0")
SCRIPTPATH=$(dirname "$SCRIPT")

cd $SCRIPTPATH && \
    docker build \
    -t edge_runtime \
    --build-arg GIT_V_TAG=$GIT_V_TAG \
    --build-arg ONNXRUNTIME_VERSION=$ONNXRUNTIME_VERSION \
    --build-arg PROFILE=dind \
    --build-arg FEATURES=$FEATURES \
    "$SCRIPTPATH/.."

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
