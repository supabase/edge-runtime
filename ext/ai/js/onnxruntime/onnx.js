const core = globalThis.Deno.core;

const DataTypeMap = Object.freeze({
  float32: Float32Array,
  float64: Float64Array,
  string: Array, // string[]
  int8: Int8Array,
  uint8: Uint8Array,
  int16: Int16Array,
  uint16: Uint16Array,
  int32: Int32Array,
  uint32: Uint32Array,
  int64: BigInt64Array,
  uint64: BigUint64Array,
  bool: Uint8Array,
});

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
    if (!Object.hasOwn(DataTypeMap, type)) {
      throw new Error(`Unsupported type: ${type}`);
    }

    const dataArray = new DataTypeMap[type](data);

    this.type = type;
    this.data = dataArray;
    this.dims = dims;
    this.size = dataArray.length
  }
}

class InferenceSession {
  sessionId;
  inputNames;
  outputNames;

  constructor(sessionId, inputNames, outputNames) {
    this.sessionId = sessionId;
    this.inputNames = inputNames;
    this.outputNames = outputNames;
  }

  static async fromBuffer(modelBuffer) {
    const [id, inputs, outputs] = await core.ops.op_sb_ai_ort_init_session(modelBuffer);

    return new InferenceSession(id, inputs, outputs);
  }

  async run(inputs) {
    const outputs = await core.ops.op_sb_ai_ort_run_session(this.sessionId, inputs);

    // Parse to Tensor
    for (const key in outputs) {
      if (Object.hasOwn(outputs, key)) {
        const { type, data, dims } = outputs[key];

        outputs[key] = new Tensor(type, data.buffer, dims);
      }
    }

    return outputs;
  }
}

const onnxruntime = {
  Tensor,
  env: {},
  InferenceSession: {
    create: InferenceSession.fromBuffer
  },
};

globalThis[Symbol.for("onnxruntime")] = onnxruntime;
