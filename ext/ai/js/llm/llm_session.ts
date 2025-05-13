import { OllamaLLMSession } from "./providers/ollama.ts";
import { OpenAILLMSession } from "./providers/openai.ts";

export type LLMRunInput = {
  /**
   * Stream response from model. Applies only for LLMs like `mistral` (default: false)
   */
  stream?: boolean;

  /**
   * Automatically abort the request to the model after specified time (in seconds). Applies only for LLMs like `mistral` (default: 60)
   */
  timeout?: number;

  prompt: string;

  signal?: AbortSignal;
};

export interface ILLMProviderMeta {
  input: ILLMProviderInput;
  output: unknown;
  options: ILLMProviderOptions;
}

export interface ILLMProviderOptions {
  model: string;
  baseURL?: string;
}

export type ILLMProviderInput<T = string | object> = T extends string ? string
  : T;

export interface ILLMProviderOutput<T = object> {
  value?: string;
  usage: {
    inputTokens: number;
    outputTokens: number;
    totalTokens: number;
  };
  inner: T;
}

export interface ILLMProvider {
  getStream(
    input: ILLMProviderInput,
    signal: AbortSignal,
  ): Promise<AsyncIterable<ILLMProviderOutput>>;
  getText(
    input: ILLMProviderInput,
    signal: AbortSignal,
  ): Promise<ILLMProviderOutput>;
}

export const providers = {
  "ollama": OllamaLLMSession,
  "openaicompatible": OpenAILLMSession,
} satisfies Record<
  string,
  new (opts: ILLMProviderOptions) => ILLMProvider & ILLMProviderMeta
>;

export type LLMProviderName = keyof typeof providers;

export type LLMProviderClass<T extends LLMProviderName> = (typeof providers)[T];
export type LLMProviderInstance<T extends LLMProviderName> = InstanceType<
  LLMProviderClass<T>
>;

export type LLMSessionRunInputOptions = {
  /**
   * Stream response from model. Applies only for LLMs like `mistral` (default: false)
   */
  stream?: boolean;

  /**
   * Automatically abort the request to the model after specified time (in seconds). Applies only for LLMs like `mistral` (default: 60)
   */
  timeout?: number;

  signal?: AbortSignal;
};

export class LLMSession {
  #inner: ILLMProvider;

  constructor(provider: ILLMProvider) {
    this.#inner = provider;
  }

  static fromProvider(name: LLMProviderName, opts: ILLMProviderOptions) {
    const ProviderType = providers[name];
    if (!ProviderType) throw new Error("invalid provider");

    const provider = new ProviderType(opts);

    return new LLMSession(provider);
  }

  run(
    input: ILLMProviderInput,
    opts: LLMSessionRunInputOptions,
  ): Promise<AsyncIterable<ILLMProviderOutput>> | Promise<ILLMProviderOutput> {
    const isStream = opts.stream ?? false;

    const timeoutSeconds = typeof opts.timeout === "number" ? opts.timeout : 60;
    const timeoutMs = timeoutSeconds * 1000;

    const timeoutSignal = AbortSignal.timeout(timeoutMs);
    const abortSignals = [opts.signal, timeoutSignal]
      .filter((it) => it instanceof AbortSignal);
    const signal = AbortSignal.any(abortSignals);

    if (isStream) {
      return this.#inner.getStream(input, signal);
    }

    return this.#inner.getText(input, signal);
  }
}
