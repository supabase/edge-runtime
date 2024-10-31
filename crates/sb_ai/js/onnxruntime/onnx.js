const core = globalThis.Deno.core;

// Workaround to serialize
BigInt64Array.prototype.toJSON = function () {
  return [...this].map(Number);
};

class Tensor {
  /** @type {number[]} Dimensions of the tensor. */
  dims;

  /** @type {DataType} Type of the tensor. */
  type;

  /** @type {DataArray} The data stored in the tensor. */
  data;

  /** @type {number} The number of elements in the tensor. */
  size;

  constructor(type, data, dims) {
    this.type = type;
    this.data = data;
    this.dims = dims;

    // console.log('onnx.js Tensor:', this);
  }
}

class InferenceSession {
  sessionId;
  inputNames;
  outputNames;

  constructor(sessionId, inputNames, outputNames) {
    this.sessionId = sessionId;
    this.inputNames = inputNames;
    this.outputNames= outputNames;
  }

  static async fromBuffer(modelBuffer) {
    const {id, inputs, outputs} = await core.ops.op_sb_ai_ort_init_session(modelBuffer);

    console.log('onnx.js fromBuffer:', {id, inputs, outputs});

    return new InferenceSession(id, inputs, outputs);
  }

  async run(inputs) {
    console.log('onnx.js run: [inputs]', inputs);

    const outputs = await core.ops.op_sb_ai_ort_run_session(this.sessionId, JSON.parse(JSON.stringify(inputs)));

    // Parse to Tensor
    for(const key in outputs) {
      if(Object.hasOwn(outputs, key)) {
        const {type, cpuData, dims} = outputs[key];
        outputs[key] = new Tensor(type, cpuData, dims);
      }
    }

    console.log('onnx.js run: [outputs]', outputs);

    return outputs;
  }
}

const onnxruntime = {
  InferenceSession: {
    create: InferenceSession.fromBuffer
  },
  Tensor,
  env: {
    wasm: {
      proxy: false
    }
  }
};

globalThis[Symbol.for("onnxruntime")] = onnxruntime;
