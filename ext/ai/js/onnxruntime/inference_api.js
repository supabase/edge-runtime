import { InferenceSession, Tensor } from 'ext:ai/onnxruntime/onnx.js';

const DEFAULT_HUGGING_FACE_OPTIONS = {
  hostname: 'https://huggingface.co',
  path: {
    template: '{REPO_ID}/resolve/{REVISION}/onnx/{MODEL_FILE}?donwload=true',
    revision: 'main',
    modelFile: 'model_quantized.onnx',
  },
};

/**
 * An user friendly API for onnx backend
 */
class UserInferenceSession {
  inner;

  id;
  inputs;
  outputs;

  constructor(session) {
    this.inner = session;

    this.id = session.sessionId;
    this.inputs = session.inputNames;
    this.outputs = session.outputNames;
  }

  static async fromUrl(modelUrl) {
    if (modelUrl instanceof URL) {
      modelUrl = modelUrl.toString();
    }

    const encoder = new TextEncoder();
    const modelUrlBuffer = encoder.encode(modelUrl);
    const session = await InferenceSession.fromBuffer(modelUrlBuffer);

    return new UserInferenceSession(session);
  }

  static async fromHuggingFace(repoId, opts = {}) {
    const hostname = opts?.hostname ?? DEFAULT_HUGGING_FACE_OPTIONS.hostname;
    const pathOpts = {
      ...DEFAULT_HUGGING_FACE_OPTIONS.path,
      ...opts?.path,
    };

    const modelPath = pathOpts.template
      .replaceAll('{REPO_ID}', repoId)
      .replaceAll('{REVISION}', pathOpts.revision)
      .replaceAll('{MODEL_FILE}', pathOpts.modelFile);

    if (!URL.canParse(modelPath, hostname)) {
      throw Error(`[Invalid URL] Couldn't parse the model path: "${modelPath}"`);
    }

    return await UserInferenceSession.fromUrl(new URL(modelPath, hostname));
  }

  async run(inputs) {
    return await this.inner.run(inputs);
  }
}

class UserTensor extends Tensor {
  constructor(type, data, dim) {
    super(type, data, dim);
  }
}

export default {
  RawSession: UserInferenceSession,
  Tensor: UserTensor,
};
