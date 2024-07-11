# syntax=docker/dockerfile:1.4
FROM rust:1.79.0-bookworm as builder
ARG TARGETPLATFORM
ARG GIT_V_VERSION
ARG ONNXRUNTIME_VERSION=1.17.0
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
    mv /usr/src/edge-runtime/target/${PROFILE}/edge-runtime /root

RUN ./scripts/download_models.sh "Supabase/gte-small" "feature-extraction"


# Application runtime without ONNX
FROM debian:bookworm-slim as edge-runtime-base
RUN apt-get update && apt-get install -y libssl-dev && rm -rf /var/lib/apt/lists/*
RUN apt-get remove -y perl && apt-get autoremove -y

COPY --from=builder /root/edge-runtime /usr/local/bin/edge-runtime
COPY --from=builder /usr/src/edge-runtime/models /etc/sb_ai/models

ENV ORT_DYLIB_PATH=/usr/local/bin/onnxruntime/lib/libonnxruntime.so
ENV SB_AI_MODELS_DIR=/etc/sb_ai/models


# ---------------------------
# ONNX Runtime provider
# Application runtime with ONNX
FROM builder as ort
RUN ./scripts/install_onnx.sh $ONNXRUNTIME_VERSION $TARGETPLATFORM /root/onnxruntime

FROM edge-runtime-base as edge-runtime
COPY --from=ort /root/onnxruntime /usr/local/bin/onnxruntime
ENTRYPOINT ["edge-runtime"]


# ---------------------------
# ONNX Runtime CUDA provider
# Application runtime with ONNX CUDA
FROM builder as ort-cuda
RUN ./scripts/install_onnx.sh $ONNXRUNTIME_VERSION $TARGETPLATFORM /root/onnxruntime --gpu

FROM nvidia/cuda:11.8.0-cudnn8-runtime-ubuntu22.04 as edge-runtime-cuda

COPY --from=edge-runtime-base /etc/sb_ai/models /etc/sb_ai/models
COPY --from=edge-runtime-base /usr/local/bin/edge-runtime /usr/local/bin/edge-runtime
COPY --from=ort-cuda /root/onnxruntime /usr/local/bin/onnxruntime

ENV ORT_DYLIB_PATH=/usr/local/bin/onnxruntime/lib/libonnxruntime.so
ENV SB_AI_MODELS_DIR=/etc/sb_ai/models

ENV NVIDIA_VISIBLE_DEVICES=all
ENV NVIDIA_DRIVER_CAPABILITIES=compute,utility

ENTRYPOINT ["edge-runtime"]

