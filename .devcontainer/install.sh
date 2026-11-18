#!/usr/bin/env bash
set -e

export $(grep -v '^#' /tmp/.env | xargs)

# Ollama
curl -fsSL https://ollama.com/install.sh | sh

# Deno
curl -fsSL https://deno.land/install.sh | bash -s -- v$DENO_VERSION

# dprint
curl -fsSL https://dprint.dev/install.sh | sh
