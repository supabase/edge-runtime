#!/usr/bin/env bash

MODEL_NAME=${1:-Supabase/gte-small}
DEFAULT_TASK=$2

MODEL_PATH="models/$(awk -F'/' '{print $2}' <<<$MODEL_NAME)"

mkdir -p $MODEL_PATH

curl -L -o $MODEL_PATH/model.onnx https://huggingface.co/$MODEL_NAME/resolve/main/onnx/model.onnx?download=true
curl -L -o $MODEL_PATH/tokenizer.json https://huggingface.co/$MODEL_NAME/resolve/main/tokenizer.json?download=true

# Creates symbolic link to default folder
if [ -n "$DEFAULT_TASK" ]; then
	mkdir -p models/defaults
	rm -f models/defaults/$DEFAULT_TASK
	ln -sr $MODEL_PATH models/defaults/$DEFAULT_TASK
fi
