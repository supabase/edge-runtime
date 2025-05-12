import "./onnxruntime/onnx.js";
import {
  LLMProviderInstance,
  LLMProviderName,
  LLMSession,
  LLMSessionRunInputOptions as LLMInputOptions,
  providers,
} from "./llm/llm_session.ts";

// @ts-ignore deno_core environment
const core = globalThis.Deno.core;

// NOTE:(kallebysantos) do we still need gte-small? Or maybe add another type 'embeddings' with custom model opt.
export type SessionType = LLMProviderName | "gte-small";

export type SessionOptions<T extends SessionType> = T extends LLMProviderName
  ? LLMProviderInstance<T>["options"]
  : never;

export type SessionInput<T extends SessionType> = T extends LLMProviderName
  ? LLMProviderInstance<T>["input"]
  : T extends "gte-small" ? string
  : never;

export type EmbeddingInputOptions = {
  /**
   * Pool embeddings by taking their mean
   */
  mean_pool?: boolean;

  /**
   * Normalize the embeddings result
   */
  normalize?: boolean;
};

export type SessionInputOptions<T extends SessionType> = T extends "gte-small"
  ? EmbeddingInputOptions
  : T extends LLMProviderName ? LLMInputOptions
  : never;

export type SessionOutput<T extends SessionType, O> = T extends "gte-small"
  ? number[]
  : T extends LLMProviderName
    ? O extends { stream: true }
      ? AsyncGenerator<LLMProviderInstance<T>["output"]>
    : LLMProviderInstance<T>["output"]
  : never;

export class Session<T extends SessionType> {
  #model?: string;
  #init?: Promise<void>;

  constructor(
    public readonly type: T,
    public readonly options?: SessionOptions<T>,
  ) {
    if (this.isEmbeddingType()) {
      this.#model = "gte-small"; // Default model
      this.#init = core.ops.op_ai_init_model(this.#model);
      return;
    }

    if (this.isLLMType()) {
      if (!Object.keys(providers).includes(type)) {
        throw new TypeError(`invalid type: '${type}'`);
      }

      if (!this.options || !this.options.model) {
        throw new Error(
          `missing required parameter 'model' for type: '${type}'`,
        );
      }

      this.options.baseURL ??= core.ops.op_get_env(
        "AI_INFERENCE_API_HOST",
      ) as string;

      if (!this.options.baseURL) {
        throw new Error(
          `missing required parameter 'baseURL' for type: '${type}'`,
        );
      }
    }
  }

  //  /** @param {string | object} prompt Either a String (ollama) or an OpenAI chat completion body object (openaicompatible): https://platform.openai.com/docs/api-reference/chat/create */
  async run<O extends SessionInputOptions<T>>(
    input: SessionInput<T>,
    options: O,
  ): Promise<SessionOutput<T, O>> {
    if (this.isLLMType()) {
      const opts = options as LLMInputOptions;
      const stream = opts.stream ?? false;

      const llmSession = LLMSession.fromProvider(this.type, {
        // safety: We did check `options` during construction
        baseURL: this.options!.baseURL,
        model: this.options!.model,
        ...this.options, // allows custom provider initialization like 'apiKey'
      });

      return await llmSession.run(input, {
        stream,
        signal: opts.signal,
        timeout: opts.timeout,
      }) as SessionOutput<T, typeof options>;
    }

    if (this.#init) {
      await this.#init;
    }

    const opts = options as EmbeddingInputOptions;

    const mean_pool = opts.mean_pool ?? true;
    const normalize = opts.normalize ?? true;

    const result = await core.ops.op_ai_run_model(
      // @ts-ignore
      this.#model,
      prompt,
      mean_pool,
      normalize,
    );

    return result;
  }

  private isEmbeddingType(
    this: Session<SessionType>,
  ): this is Session<"gte-small"> {
    return this.type === "gte-small";
  }

  private isLLMType(
    this: Session<SessionType>,
  ): this is Session<LLMProviderName> {
    return this.type !== "gte-small";
  }
}

const MAIN_WORKER_API = {
  tryCleanupUnusedSession: () =>
    /* async */ core.ops.op_ai_try_cleanup_unused_session(),
};

const USER_WORKER_API = {
  Session,
};

export { MAIN_WORKER_API, USER_WORKER_API };
