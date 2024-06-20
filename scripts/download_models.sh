#!/usr/bin/env bash

MODEL_NAME=${1:-Supabase/gte-small}
DEFAULT_TASK=$2

MODEL_PATH="models/$(awk -F'/' '{print $2}' <<<$MODEL_NAME)"

mkdir -p $MODEL_PATH

MODEL_FILENAME="model.onnx"

# Downloads the float 16 version
if [[ $* == *"--f16"* ]]; then
	MODEL_FILENAME="model_fp16.onnx"

# Downloads the quantized version
elif [[ $* == *"--quantized"* ]]; then
	MODEL_FILENAME="model_quantized.onnx"
fi

curl -L -o $MODEL_PATH/model.onnx https://huggingface.co/$MODEL_NAME/resolve/main/onnx/$MODEL_FILENAME?download=true
curl -L -o $MODEL_PATH/tokenizer.json https://huggingface.co/$MODEL_NAME/resolve/main/tokenizer.json?download=true

# Creates symbolic link to default folder
if [ -n "$DEFAULT_TASK" ]; then
	mkdir -p models/defaults
	rm -f models/defaults/$DEFAULT_TASK
	ln -sr $MODEL_PATH models/defaults/$DEFAULT_TASK
fi
