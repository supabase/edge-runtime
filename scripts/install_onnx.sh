#!/usr/bin/env bash

ONNX_VERSION=${1:-1.17.3}
TARGETPLATFORM=$2
SAVE_PATH=${3:-"./onnx-runtime"}

ONNX_DOWNLOAD_FILE="onnxruntime-linux"
ONNX_TARGET_PLATFORM=$([ "$TARGETPLATFORM" == "linux/arm64" ] && echo "aarch64" || echo "x64")

if [[ $* == *"--gpu"* ]]; then
	ONNX_DOWNLOAD_FILE="$ONNX_DOWNLOAD_FILE-$ONNX_TARGET_PLATFORM-gpu-$ONNX_VERSION"
else
	ONNX_DOWNLOAD_FILE="$ONNX_DOWNLOAD_FILE-$ONNX_TARGET_PLATFORM-$ONNX_VERSION"
fi

wget -qO- "https://github.com/microsoft/onnxruntime/releases/download/v${ONNX_VERSION}/${ONNX_DOWNLOAD_FILE}.tgz" | tar zxv

mv $ONNX_DOWNLOAD_FILE $SAVE_PATH
