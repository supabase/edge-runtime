FROM mcr.microsoft.com/devcontainers/rust:dev-1-bookworm

ARG TARGETPLATFORM
ARG ONNXRUNTIME_VERSION

RUN apt-get update && apt-get install -y build-essential cmake libclang-dev lldb \
    nodejs npm hyperfine

COPY scripts/install_onnx.sh /tmp/install_onnx.sh
COPY scripts/download_models.sh /tmp/download_models.sh

WORKDIR /tmp
RUN ./install_onnx.sh $ONNXRUNTIME_VERSION $TARGETPLATFORM /usr/local/bin/libonnxruntime.so
RUN ./download_models.sh
RUN mkdir -p /etc/sb_ai && cp -r /tmp/models /etc/sb_ai/models

ENV ORT_DYLIB_PATH=/usr/local/bin/libonnxruntime.so
ENV SB_AI_MODELS_DIR=/etc/sb_ai/models