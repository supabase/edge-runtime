import {
  ILLMProvider,
  ILLMProviderInput,
  ILLMProviderMeta,
  ILLMProviderOptions,
} from '../llm_session.ts';
import { parseJSON } from '../utils/json_parser.ts';

export type OllamaProviderOptions = ILLMProviderOptions;
export type OllamaProviderInput = ILLMProviderInput<string>;

export type OllamaMessage = {
  model: string;
  created_at: Date;
  response: string;
  done: boolean;
  context: number[];
  total_duration: number;
  load_duration: number;
  prompt_eval_count: number;
  prompt_eval_duration: number;
  eval_count: number;
  eval_duration: number;
};

export class OllamaLLMSession implements ILLMProvider, ILLMProviderMeta {
  input!: OllamaProviderInput;
  output!: unknown;
  options: OllamaProviderOptions;

  constructor(opts: OllamaProviderOptions) {
    this.options = opts;
  }

  // ref: https://github.com/ollama/ollama-js/blob/6a4bfe3ab033f611639dfe4249bdd6b9b19c7256/src/utils.ts#L26
  async getStream(
    prompt: OllamaProviderInput,
    signal: AbortSignal,
  ): Promise<AsyncIterable<OllamaMessage>> {
    const generator = await this.generate(
      prompt,
      signal,
      true,
    ) as AsyncGenerator<OllamaMessage>;

    const stream = async function* () {
      for await (const message of generator) {
        if ('error' in message) {
          if (message.error instanceof Error) {
            throw message.error;
          } else {
            throw new Error(message.error as string);
          }
        }

        yield message;
        if (message.done) {
          return;
        }
      }

      throw new Error(
        'Did not receive done or success response in stream.',
      );
    };

    return stream();
  }

  async getText(
    prompt: OllamaProviderInput,
    signal: AbortSignal,
  ): Promise<OllamaMessage> {
    const response = await this.generate(prompt, signal) as OllamaMessage;

    if (!response?.done) {
      throw new Error('Expected a completed response.');
    }

    return response;
  }

  private async generate(
    prompt: string,
    signal: AbortSignal,
    stream: boolean = false,
  ) {
    const res = await fetch(
      new URL('/api/generate', this.options.baseURL),
      {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({
          model: this.options.model,
          stream,
          prompt,
        }),
        signal,
      },
    );

    if (!res.ok) {
      throw new Error(
        `Failed to fetch inference API host. Status ${res.status}: ${res.statusText}`,
      );
    }

    if (!res.body) {
      throw new Error('Missing body');
    }

    if (stream) {
      return parseJSON<OllamaMessage>(res.body, signal);
    }

    const result: OllamaMessage = await res.json();

    return result;
  }
}
