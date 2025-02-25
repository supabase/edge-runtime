const core = globalThis.Deno.core;
import { InferenceSession, Tensor } from "ext:ai/onnxruntime/onnx.js";

const DEFAULT_HUGGING_FACE_OPTIONS = {
  hostname: "https://huggingface.co",
  path: {
    template: "{REPO_ID}/resolve/{REVISION}/onnx/{MODEL_FILE}?donwload=true",
    revision: "main",
    modelFile: "model_quantized.onnx",
  },
};

const DEFAULT_STORAGE_OPTIONS = () => ({
  hostname: Deno.env.get("SUPABASE_URL"),
  mode: {
    authorization: Deno.env.get("SUPABASE_SERVICE_ROLE_KEY"),
  },
});

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

  static async fromUrl(modelUrl, authorization) {
    if (modelUrl instanceof URL) {
      modelUrl = modelUrl.toString();
    }

    const encoder = new TextEncoder();
    const modelUrlBuffer = encoder.encode(modelUrl);
    const session = await InferenceSession.fromRequest(
      modelUrlBuffer,
      authorization,
    );

    return new UserInferenceSession(session);
  }

  static async fromHuggingFace(repoId, opts = {}) {
    const hostname = opts?.hostname ?? DEFAULT_HUGGING_FACE_OPTIONS.hostname;
    const pathOpts = {
      ...DEFAULT_HUGGING_FACE_OPTIONS.path,
      ...opts?.path,
    };

    const modelPath = pathOpts.template
      .replaceAll("{REPO_ID}", repoId)
      .replaceAll("{REVISION}", pathOpts.revision)
      .replaceAll("{MODEL_FILE}", pathOpts.modelFile);

    if (!URL.canParse(modelPath, hostname)) {
      throw Error(
        `[Invalid URL] Couldn't parse the model path: "${modelPath}"`,
      );
    }

    return await UserInferenceSession.fromUrl(new URL(modelPath, hostname));
  }

  static async fromStorage(modelPath, opts = {}) {
    const defaultOpts = DEFAULT_STORAGE_OPTIONS();
    const hostname = opts?.hostname ?? defaultOpts.hostname;
    const mode = opts?.mode ?? defaultOpts.mode;

    const assetPath = mode === "public"
      ? `public/${modelPath}`
      : `authenticated/${modelPath}`;

    const storageUrl = `/storage/v1/object/${assetPath}`;

    if (!URL.canParse(storageUrl, hostname)) {
      throw Error(
        `[Invalid URL] Couldn't parse the model path: "${storageUrl}"`,
      );
    }

    return await UserInferenceSession.fromUrl(
      new URL(storageUrl, hostname),
      mode?.authorization,
    );
  }
  async run(inputs) {
    const outputs = await core.ops.op_ai_ort_run_session(this.id, inputs);

    // Parse to Tensor
    for (const key in outputs) {
      if (Object.hasOwn(outputs, key)) {
        const { type, data, dims } = outputs[key];

        outputs[key] = new UserTensor(type, data.buffer, dims);
      }
    }

    return outputs;
  }
}

class UserTensor extends Tensor {
  constructor(type, data, dim) {
    super(type, data, dim);
  }

  async tryEncodeAudio(sampleRate) {
    return await core.ops.op_ai_ort_encode_tensor_audio(this.data, sampleRate);
  }
}

export default {
  RawSession: UserInferenceSession,
  RawTensor: UserTensor,
};
