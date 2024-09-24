# syntax=docker/dockerfile:1.4

FROM rust:1.79.0-bookworm as builder

ARG TARGETPLATFORM
ARG GIT_V_VERSION
ARG ONNXRUNTIME_VERSION=1.17.0
ARG PROFILE=release
ARG FEATURES

RUN apt-get update && apt-get install -y llvm-dev libclang-dev clang cmake binutils

WORKDIR /usr/src/edge-runtime

COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=${TARGETPLATFORM} --mount=type=cache,target=/usr/src/edge-runtime/target,id=${TARGETPLATFORM} \
    GIT_V_TAG=${GIT_V_VERSION} cargo build --profile ${PROFILE} --features "${FEATURES}" && \
    mv /usr/src/edge-runtime/target/${PROFILE}/edge-runtime /root

RUN objcopy --compress-debug-sections \
    --only-keep-debug \
    /root/edge-runtime \
    /root/edge-runtime.debug
RUN objcopy --strip-debug \
    --add-gnu-debuglink=/root/edge-runtime.debug \
    /root/edge-runtime

RUN ./scripts/install_onnx.sh $ONNXRUNTIME_VERSION $TARGETPLATFORM /root/libonnxruntime.so
RUN ./scripts/download_models.sh

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y libssl-dev && rm -rf /var/lib/apt/lists/*
RUN apt-get remove -y perl && apt-get autoremove -y

COPY --from=builder /root/edge-runtime /usr/local/bin/edge-runtime
COPY --from=builder /root/edge-runtime.debug /usr/local/bin/edge-runtime.debug
COPY --from=builder /root/libonnxruntime.so /usr/local/bin/libonnxruntime.so
COPY --from=builder /usr/src/edge-runtime/models /etc/sb_ai/models

ENV ORT_DYLIB_PATH=/usr/local/bin/libonnxruntime.so
ENV SB_AI_MODELS_DIR=/etc/sb_ai/models

ENTRYPOINT ["edge-runtime"]
