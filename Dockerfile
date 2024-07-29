# syntax=docker/dockerfile:1.4
FROM rust:1.79.0-bookworm as builder
ARG TARGETPLATFORM
ARG GIT_V_VERSION
ARG ONNXRUNTIME_VERSION=1.17.3
ARG PROFILE=release
ARG FEATURES

RUN apt-get update && apt-get install -y llvm-dev libclang-dev clang cmake
WORKDIR /usr/src/edge-runtime

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=${TARGETPLATFORM} \
    cargo install cargo-strip
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=${TARGETPLATFORM} --mount=type=cache,target=/usr/src/edge-runtime/target,id=${TARGETPLATFORM} \
    GIT_V_TAG=${GIT_V_VERSION} cargo build --profile ${PROFILE} --features "${FEATURES}" && \
    cargo strip && \
    mv /usr/src/edge-runtime/target/${PROFILE}/edge-runtime /root && \
    mv /usr/src/edge-runtime/target/${PROFILE}/preload-sb-ai-defs /root


# Application runtime without ONNX
FROM debian:bookworm-slim as edge-runtime-base

RUN apt-get update && apt-get install -y libssl-dev && rm -rf /var/lib/apt/lists/*
RUN apt-get remove -y perl && apt-get autoremove -y

COPY --from=builder /root/edge-runtime /usr/local/bin/edge-runtime

ENV ORT_DYLIB_PATH=/usr/local/bin/onnxruntime/lib/libonnxruntime.so


# ONNX Runtime provider
# Application runtime with ONNX
FROM builder as ort

RUN ./scripts/install_onnx.sh $ONNXRUNTIME_VERSION $TARGETPLATFORM /root/onnxruntime


# Preload defs
FROM ort as preload-defs

COPY --from=builder /root/preload-sb-ai-defs /usr/local/bin/preload-sb-ai-defs

ENV ORT_DYLIB_PATH=/root/onnxruntime/lib/libonnxruntime.so
RUN --mount=type=cache,target=/root/.cache/ort.pyke.io,id=${TARGETPLATFORM} \
    RUST_LOG=info /usr/local/bin/preload-sb-ai-defs && \
    mkdir -p /root/preload-defs-cache && \
    cp -r /root/.cache/ort.pyke.io/models /root/preload-defs-cache && \
    cp -r /root/.cache/ort.pyke.io/tokenizers /root/preload-defs-cache


# ONNX Runtime CUDA provider
# Application runtime with ONNX CUDA
FROM builder as ort-cuda

RUN ./scripts/install_onnx.sh $ONNXRUNTIME_VERSION $TARGETPLATFORM /root/onnxruntime --gpu


# With CUDA
FROM nvidia/cuda:11.8.0-cudnn8-runtime-ubuntu22.04 as edge-runtime-cuda

COPY --from=edge-runtime-base /usr/local/bin/edge-runtime /usr/local/bin/edge-runtime
COPY --from=ort-cuda /root/onnxruntime /usr/local/bin/onnxruntime
COPY --from=preload-defs /root/preload-defs-cache /root/.cache/ort.pyke.io

ENV ORT_DYLIB_PATH=/usr/local/bin/onnxruntime/lib/libonnxruntime.so
ENV NVIDIA_VISIBLE_DEVICES=all
ENV NVIDIA_DRIVER_CAPABILITIES=compute,utility

ENTRYPOINT ["edge-runtime"]


# Base
FROM edge-runtime-base as edge-runtime

COPY --from=ort /root/onnxruntime /usr/local/bin/onnxruntime
COPY --from=preload-defs /root/preload-defs-cache /root/.cache/ort.pyke.io

ENTRYPOINT ["edge-runtime"]
