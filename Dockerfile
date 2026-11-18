# syntax=docker/dockerfile:1.4

FROM rust:1.98.0-bookworm as builder

ARG TARGETPLATFORM
ARG GIT_V_TAG
ARG PROFILE=release
ARG FEATURES

RUN apt-get update && apt-get install -y llvm-dev libclang-dev clang cmake binutils

WORKDIR /usr/src/edge-runtime

COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=${TARGETPLATFORM} --mount=type=cache,target=/usr/src/edge-runtime/target,id=${TARGETPLATFORM} \
    GIT_V_TAG=${GIT_V_TAG} cargo build --profile ${PROFILE} --features "${FEATURES}" && \
    mv /usr/src/edge-runtime/target/${PROFILE}/edge-runtime /root

RUN objcopy --compress-debug-sections \
    --only-keep-debug \
    /root/edge-runtime \
    /root/edge-runtime.debug
RUN objcopy --strip-debug \
    --add-gnu-debuglink=/root/edge-runtime.debug \
    /root/edge-runtime


# Application runtime
FROM debian:bookworm-slim as edge-runtime

RUN apt-get update && apt-get install -y libssl-dev \
    ca-certificates \
    && update-ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN apt-get remove -y perl && apt-get autoremove -y

COPY --from=builder /root/edge-runtime /usr/local/bin/edge-runtime
COPY --from=builder /root/edge-runtime.debug /usr/local/bin/edge-runtime.debug

ENTRYPOINT ["edge-runtime"]
