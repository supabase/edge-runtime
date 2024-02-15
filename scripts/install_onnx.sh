#!/usr/bin/env bash

curl -O https://registry.npmjs.org/onnxruntime-node/-/onnxruntime-node-$1.tgz && tar zxvf onnxruntime-node-$1.tgz

if [ "$2" == "linux/arm64" ]; then
  mv ./package/bin/napi-v3/linux/arm64/libonnxruntime.so.$1 $3
else
  mv ./package/bin/napi-v3/linux/x64/libonnxruntime.so.$1 $3
fi
