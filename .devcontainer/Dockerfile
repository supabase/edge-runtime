FROM mcr.microsoft.com/devcontainers/rust:dev-1-bookworm

ARG TARGETPLATFORM
ARG ONNXRUNTIME_VERSION
ARG DENO_VERSION

RUN apt-get update && apt-get install -y build-essential cmake libclang-dev lldb \
    nodejs npm hyperfine

COPY scripts/install_onnx.sh /tmp/install_onnx.sh

WORKDIR /tmp
RUN ./install_onnx.sh $ONNXRUNTIME_VERSION $TARGETPLATFORM /usr/local/bin/onnxruntime

ENV ORT_DYLIB_PATH=/usr/local/bin/onnxruntime/lib/libonnxruntime.so

# Ollama
RUN curl -fsSL https://ollama.com/install.sh | sh

# Deno
RUN curl -fsSL https://deno.land/install.sh | sh
RUN deno upgrade --version $DENO_VERSION