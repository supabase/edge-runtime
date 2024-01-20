FROM mcr.microsoft.com/devcontainers/rust:1-1-bookworm
RUN apt-get update && apt-get install -y build-essential cmake libclang-dev lldb
RUN wget https://github.com/tsenart/vegeta/releases/download/v12.11.1/vegeta_12.11.1_linux_arm64.tar.gz
RUN tar -xvf vegeta_12.11.1_linux_arm64.tar.gz
RUN mv ./vegeta /usr/local/bin/vegeta