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
  inferenceAPIHost: string;
  model: string;
}

export interface ILLMProvider {
  // TODO:(kallebysantos) remove 'any'
  getStream(prompt: string, signal: AbortSignal): Promise<AsyncIterable<any>>;
  getText(prompt: string, signal: AbortSignal): Promise<any>;
}

export class LLMSession {
  #inner: ILLMProvider;

  constructor(provider: ILLMProvider) {
    this.#inner = provider;
  }

  static fromProvider(name: string, opts: ILLMProviderOptions) {
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

    if (isStream) {
      return this.#inner.getStream(opts.prompt, signal);
    }

    return this.#inner.getText(opts.prompt, signal);
  }
}

