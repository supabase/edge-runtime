#!/usr/bin/env bash

mkdir -p models/gte-small
cd models/gte-small
curl -L -o model.onnx https://huggingface.co/Supabase/gte-small/resolve/main/onnx/model.onnx?download=true
curl -L -o tokenizer.json https://huggingface.co/Supabase/gte-small/resolve/main/tokenizer.json?download=true
