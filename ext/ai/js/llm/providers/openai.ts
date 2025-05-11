import { ILLMProvider, ILLMProviderInput, ILLMProviderOptions } from '../llm_session.ts';
import { parseJSONOverEventStream } from '../utils/json_parser.ts';

export type OpenAIProviderOptions = ILLMProviderOptions & {
  apiKey?: string;
};

// TODO:(kallebysantos) need to double check theses AI generated types
export type OpenAIRequest = {
  model: string;
  messages: {
    role: 'system' | 'user' | 'assistant' | 'tool';
    content: string;
    name?: string;
    tool_call_id?: string;
    function_call?: {
      name: string;
      arguments: string;
    };
  }[];
  temperature?: number;
  top_p?: number;
  n?: number;
  stream?: boolean;
  stop?: string | string[];
  max_tokens?: number;
  presence_penalty?: number;
  frequency_penalty?: number;
  logit_bias?: { [token: string]: number };
  user?: string;
  tools?: {
    type: 'function';
    function: {
      name: string;
      description?: string;
      parameters: any; // Can be refined based on your function definition
    };
  }[];
  tool_choice?: 'none' | 'auto' | {
    type: 'function';
    function: { name: string };
  };
};

export type OpenAIResponse = {
  id: string;
  object: 'chat.completion';
  created: number;
  model: string;
  system_fingerprint?: string;
  choices: {
    index: number;
    message: {
      role: 'assistant' | 'user' | 'system' | 'tool';
      content: string | null;
      function_call?: {
        name: string;
        arguments: string;
      };
      tool_calls?: {
        id: string;
        type: 'function';
        function: {
          name: string;
          arguments: string;
        };
      }[];
    };
    finish_reason: 'stop' | 'length' | 'tool_calls' | 'content_filter' | null;
  }[];
  usage?: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
};

export type OpenAIInput = Omit<OpenAIRequest, 'stream' | 'model'>;

export type OpenAIProviderInput = Omit<ILLMProviderInput, 'prompt'> & {
  // Use Open AI defs
  prompt: OpenAIInput;
};

export class OpenAILLMSession implements ILLMProvider {
  opts: OpenAIProviderOptions;

  constructor(opts: OpenAIProviderOptions) {
    this.opts = opts;
  }

  async getStream(
    { prompt, signal }: OpenAIProviderInput,
  ): Promise<AsyncIterable<OpenAIResponse>> {
    const generator = await this.generate(
      prompt,
      signal,
      true,
    ) as AsyncGenerator<any>; // TODO:(kallebysantos) remove any

    const stream = async function* () {
      for await (const message of generator) {
        // TODO:(kallebysantos) Simplify duplicated code for stream error checking
        if ('error' in message) {
          if (message.error instanceof Error) {
            throw message.error;
          } else {
            throw new Error(message.error as string);
          }
        }

        yield message;
        const finishReason = message.choices[0].finish_reason;

        if (finishReason) {
          if (finishReason !== 'stop') {
            throw new Error('Expected a completed response.');
          }

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
    { prompt, signal }: OpenAIProviderInput,
  ): Promise<OpenAIResponse> {
    const response = await this.generate(
      prompt,
      signal,
    ) as OpenAIResponse;

    const finishReason = response.choices[0].finish_reason;

    if (finishReason !== 'stop') {
      throw new Error('Expected a completed response.');
    }

    return response;
  }

  private async generate(
    input: OpenAIInput,
    signal: AbortSignal,
    stream: boolean = false,
  ) {
    const res = await fetch(
      new URL('/v1/chat/completions', this.opts.inferenceAPIHost),
      {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'Authorization': `Bearer ${this.opts.apiKey}`,
        },
        body: JSON.stringify(
          {
            ...input,
            model: this.opts.model,
            stream,
          } satisfies OpenAIRequest,
        ),
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
      return parseJSONOverEventStream(res.body, signal);
    }

    const result: OpenAIResponse = await res.json();

    return result;
  }
}
