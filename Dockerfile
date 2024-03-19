# syntax=docker/dockerfile:1.4
FROM rust:1.74.1-bookworm as builder

ARG TARGETPLATFORM
ARG GIT_V_VERSION
ARG ONNXRUNTIME_VERSION=1.17.0

RUN apt-get update && apt-get install -y llvm-dev libclang-dev clang cmake
WORKDIR /usr/src/edge-runtime
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=${TARGETPLATFORM} \
    cargo install cargo-strip
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=${TARGETPLATFORM} --mount=type=cache,target=/usr/src/edge-runtime/target,id=${TARGETPLATFORM} \
    GIT_V_TAG=${GIT_V_VERSION} cargo build --release && \
    cargo strip && \
    mv /usr/src/edge-runtime/target/release/edge-runtime /root
RUN ./scripts/install_onnx.sh $ONNXRUNTIME_VERSION $TARGETPLATFORM /root/libonnxruntime.so


FROM debian:bookworm-slim

ARG TARGETARCH
ARG USE_CODE_SERVER_INTEGRATION

ENV USE_CODE_SERVER_INTEGRATION ${USE_CODE_SERVER_INTEGRATION}
ENV DENO_VERSION 1.40.3
ENV CODE_SERVER_VERSION 4.20.0
ENV CODE_SERVER_HOST 0.0.0.0
ENV CODE_SERVER_PORT 8999
ENV CODE_SERVER_EXTENSIONS denoland.vscode-deno
ENV ORT_DYLIB_PATH=/usr/local/bin/libonnxruntime.so
ENV SB_AI_MODELS_DIR=/etc/sb_ai/models

RUN apt-get update && apt-get install -y libssl-dev && rm -rf /var/lib/apt/lists/*
RUN apt-get remove -y perl && apt-get autoremove -y

COPY --from=builder /root/edge-runtime /usr/local/bin/edge-runtime
COPY --from=builder /root/libonnxruntime.so /usr/local/bin/libonnxruntime.so
COPY ./models /etc/sb_ai/models
COPY ./bin/entrypoint.sh /usr/local/bin

RUN chmod u+x /usr/local/bin/entrypoint.sh

# vscode-server integration
RUN if [ -n "$USE_CODE_SERVER_INTEGRATION" ]; then \
    apt-get update && apt-get install -y ca-certificates curl wget unzip dumb-init \
    && wget https://github.com/coder/code-server/releases/download/v${CODE_SERVER_VERSION}/code-server_${CODE_SERVER_VERSION}_${TARGETARCH}.deb -P /tmp \
    && dpkg -i /tmp/code-server_${CODE_SERVER_VERSION}_${TARGETARCH}.deb \
    && rm -f /tmp/code-server_${CODE_SERVER_VERSION}_${TARGETARCH}.deb; \
    if [ "${TARGETARCH}" = "arm64" ]; then \
    wget https://github.com/LukeChannings/deno-arm64/releases/download/v${DENO_VERSION}/deno-linux-arm64.zip -P /tmp \
    && unzip /tmp/deno-linux-arm64.zip -d /usr/local/bin; \
    else \
    curl -fsSL https://deno.land/install.sh | DENO_INSTALL=/usr/local sh; \
    fi \
    fi

ENTRYPOINT ["dumb-init", "--", "/usr/local/bin/entrypoint.sh"]
