import { OllamaLLMSession } from './providers/ollama.ts';
import { OpenAILLMSession } from './providers/openai.ts';

// @ts-ignore deno_core environment
const core = globalThis.Deno.core;

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

export interface ILLMProviderOptions {
  model: string;
  inferenceAPIHost: string;
}

export interface ILLMProviderInput {
  prompt: string | object;
  signal: AbortSignal;
}

export interface ILLMProvider {
  // TODO:(kallebysantos) remove 'any'
  // TODO: (kallebysantos) standardised output format
  getStream(input: ILLMProviderInput): Promise<AsyncIterable<any>>;
  getText(input: ILLMProviderInput): Promise<any>;
}

export const providers = {
  'ollama': OllamaLLMSession,
  'openaicompatible': OpenAILLMSession,
} satisfies Record<string, new (opts: ILLMProviderOptions) => ILLMProvider>;

export type LLMProviderName = keyof typeof providers;

export class LLMSession {
  #inner: ILLMProvider;

  constructor(provider: ILLMProvider) {
    this.#inner = provider;
  }

  static fromProvider(name: LLMProviderName, opts: ILLMProviderOptions) {
    const ProviderType = providers[name];
    if (!ProviderType) throw new Error('invalid provider');

    const provider = new ProviderType(opts);

    return new LLMSession(provider);
  }

  run(
    opts: LLMRunInput,
  ): Promise<AsyncIterable<any>> | Promise<any> {
    const isStream = opts.stream ?? false;

    const timeoutSeconds = typeof opts.timeout === 'number' ? opts.timeout : 60;
    const timeoutMs = timeoutSeconds * 1000;

    const timeoutSignal = AbortSignal.timeout(timeoutMs);
    const abortSignals = [opts.signal, timeoutSignal]
      .filter((it) => it instanceof AbortSignal);
    const signal = AbortSignal.any(abortSignals);

    const llmInput: ILLMProviderInput = { prompt: opts.prompt, signal };
    if (isStream) {
      return this.#inner.getStream(llmInput);
    }

    return this.#inner.getText(llmInput);
  }
}
