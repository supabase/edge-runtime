const core = globalThis.Deno.core;

class Tensor {
  /** @type {DataType} Type of the tensor. */
  type;

  /** @type {DataArray} The data stored in the tensor. */
  data;

  /** @type {number[]} Dimensions of the tensor. */
  dims;

  /** @type {number} The number of elements in the tensor. */
  size = 0;

  constructor(type, data, dims) {
    this.type = type;
    this.data = data;
    this.dims = dims;
  }

  static isTensorLike(object) {
    return (
      Object.hasOwn(object, 'type')
      && Object.hasOwn(object, 'cpuData')
      && Object.hasOwn(object, 'dims')
    )
  }

  static toTuple(tensorLike) {
    if (!this.isTensorLike(tensorLike)) {
      throw Error('The given object is not a valid Tensor like.');
    }

    return [tensorLike.type, tensorLike.cpuData, tensorLike.dims]
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
    const [id, inputs, outputs] = await core.ops.op_sb_ai_ort_init_session(modelBuffer);

    return new InferenceSession(id, inputs, outputs);
  }

  async run(inputs) {
    // We pass values as tuples to avoid string allocation
    // https://docs.rs/deno_core/latest/deno_core/convert/trait.ToV8.html#structs
    const tupledTensors = Object.values(inputs).map(tensor => Tensor.toTuple(tensor));
    const outputTuples = await core.ops.op_sb_ai_ort_run_session(this.sessionId, tupledTensors);

    // Since we got outputs as tuples we need to re-map it to an object
    const result = {};
    for (let idx = 0; idx < this.outputNames.length; idx++) {
      const key = this.outputNames[idx];
      const [type, data, dims] = outputTuples[idx];

      result[key] = new Tensor(type, data, dims);
    }

    return result;
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
