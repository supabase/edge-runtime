#!/usr/bin/env bash
set -e

TARGETPLATFORM=$1

export $(grep -v '^#' /tmp/.env | xargs)

# ONNX Runtime
/tmp/install_onnx.sh $ONNXRUNTIME_VERSION $TARGETPLATFORM /tmp/onnxruntime
mv /tmp/onnxruntime/lib/libonnxruntime.so* /usr/lib
/tmp/download_models.sh
mkdir -p /etc/sb_ai && cp -r /tmp/models /etc/sb_ai/models

# Ollama
curl -fsSL https://ollama.com/install.sh | sh

# Deno
mkdir -p /deno
curl -fsSL https://deno.land/install.sh | bash -s -- v$DENO_VERSION
chown -R vscode /deno