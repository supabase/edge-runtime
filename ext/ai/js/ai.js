import 'ext:ai/onnxruntime/onnx.js';
import { LLMSession, providers } from './llm/llm_session.ts';

const core = globalThis.Deno.core;

class Session {
  model;
  init;
  is_ext_inference_api;
  inferenceAPIHost;
  extraOpts;

  // TODO:(kallebysantos) get 'provider' type here and use type checking to suggest Inputs when run
  constructor(model, opts = {}) {
    this.model = model;
    this.is_ext_inference_api = false;
    this.extraOpts = opts;

    // TODO:(kallebysantos) do we still need gte-small?
    if (model === 'gte-small') {
      this.init = core.ops.op_ai_init_model(model);
    } else {
      this.inferenceAPIHost = core.ops.op_get_env('AI_INFERENCE_API_HOST');
      this.is_ext_inference_api = !!this.inferenceAPIHost; // only enable external inference API if env variable is set
    }
  }

  /** @param {string | object} prompt Either a String (ollama) or an OpenAI chat completion body object (openaicompatible): https://platform.openai.com/docs/api-reference/chat/create */
  async run(prompt, opts = {}) {
    if (this.is_ext_inference_api) {
      const stream = opts.stream ?? false;

      /** @type {'ollama' | 'openaicompatible'} */
      // TODO:(kallebysantos) get mode from 'new' and apply type checking based on that
      const mode = opts.mode ?? 'ollama';

      if (!Object.keys(providers).includes(mode)) {
        throw new TypeError(`invalid mode: ${mode}`);
      }

      const llmSession = LLMSession.fromProvider(mode, {
        inferenceAPIHost: this.inferenceAPIHost,
        model: this.model,
        ...this.extraOpts, // allows custom provider initialization like 'apiKey'
      });

      return await llmSession.run({
        prompt,
        stream,
        signal: opts.signal,
        timeout: opts.timeout,
      });
    }

    if (this.init) {
      await this.init;
    }

    const mean_pool = opts.mean_pool ?? true;
    const normalize = opts.normalize ?? true;
    const result = await core.ops.op_ai_run_model(
      this.model,
      prompt,
      mean_pool,
      normalize,
    );

    return result;
  }
}

const MAIN_WORKER_API = {
  tryCleanupUnusedSession: () => /* async */ core.ops.op_ai_try_cleanup_unused_session(),
};

const USER_WORKER_API = {
  Session,
};

export { MAIN_WORKER_API, USER_WORKER_API };
