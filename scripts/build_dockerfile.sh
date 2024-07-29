#!/usr/bin/env bash

PWD=$(pwd)
PROFILE=${1:-dind}
SCRIPT=$(readlink -f "$0")
SCRIPTPATH=$(dirname "$SCRIPT")

source $SCRIPTPATH/docker_vars.sh
cd $SCRIPTPATH && \
    docker build \
    -t edge_runtime \
    --build-arg GIT_V_TAG=$GIT_V_TAG \
    --build-arg ONNXRUNTIME_VERSION=$ONNXRUNTIME_VERSION \
    --build-arg PROFILE=$PROFILE \
    --build-arg FEATURES=$FEATURES \
    "$SCRIPTPATH/.."

cd $PWD