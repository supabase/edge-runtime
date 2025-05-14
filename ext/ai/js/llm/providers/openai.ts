import { Result } from "../../ai.ts";
import {
  ILLMProvider,
  ILLMProviderError,
  ILLMProviderInput,
  ILLMProviderMeta,
  ILLMProviderOptions,
  ILLMProviderOutput,
} from "../llm_session.ts";
import { parseJSONOverEventStream } from "../utils/json_parser.ts";

export type OpenAIProviderOptions = ILLMProviderOptions & {
  apiKey?: string;
};

// NOTE:(kallebysantos) we define all types here for better development as well avoid `"npm:openai"` import
// TODO:(kallebysantos) need to double check theses AI generated types
export type OpenAIRequest = {
  model: string;
  messages: {
    // NOTE:(kallebysantos) using role as union type is great for intellisense suggestions
    // but at same time it forces users to `{} satisfies Supabase.ai.OpenAICompatibleInput`
    role: "system" | "user" | "assistant" | "tool";
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
  stream_options: {
    include_usage: boolean;
  };
  stop?: string | string[];
  max_tokens?: number;
  presence_penalty?: number;
  frequency_penalty?: number;
  logit_bias?: { [token: string]: number };
  user?: string;
  tools?: {
    type: "function";
    function: {
      name: string;
      description?: string;
      parameters: unknown;
    };
  }[];
  tool_choice?: "none" | "auto" | {
    type: "function";
    function: { name: string };
  };
};

export type OpenAIResponseUsage = {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
  prompt_tokens_details: {
    cached_tokens: number;
    audio_tokens: number;
  };
  completion_tokens_details: {
    reasoning_tokens: number;
    audio_tokens: number;
    accepted_prediction_tokens: number;
    rejected_prediction_tokens: number;
  };
};

export type OpenAIResponseChoice = {
  index: number;
  message?: {
    role: "assistant" | "user" | "system" | "tool";
    content: string | null;
    function_call?: {
      name: string;
      arguments: string;
    };
    tool_calls?: {
      id: string;
      type: "function";
      function: {
        name: string;
        arguments: string;
      };
    }[];
  };
  delta?: {
    content: string | null;
  };
  finish_reason: "stop" | "length" | "tool_calls" | "content_filter" | null;
};

export type OpenAIResponse = {
  id: string;
  object: "chat.completion";
  created: number;
  model: string;
  system_fingerprint?: string;
  choices: OpenAIResponseChoice[];
  usage?: OpenAIResponseUsage;
};

export type OpenAICompatibleInput = Omit<
  OpenAIRequest,
  "stream" | "stream_options" | "model"
>;

export type OpenAIProviderInput = ILLMProviderInput<OpenAICompatibleInput>;
export type OpenAIProviderOutput = Result<
  ILLMProviderOutput<OpenAIResponse>,
  OpenAIProviderError
>;
export type OpenAIProviderError = ILLMProviderError<object>;

export class OpenAILLMSession implements ILLMProvider, ILLMProviderMeta {
  input!: OpenAIProviderInput;
  output!: OpenAIProviderOutput;
  error!: OpenAIProviderError;
  options: OpenAIProviderOptions;

  constructor(opts: OpenAIProviderOptions) {
    this.options = opts;
  }

  async getStream(
    prompt: OpenAIProviderInput,
    signal: AbortSignal,
  ): Promise<
    Result<AsyncIterable<OpenAIProviderOutput>, OpenAIProviderError>
  > {
    const [generator, error] = await this.generate(
      prompt,
      signal,
      true,
    ) as Result<AsyncGenerator<OpenAIResponse>, OpenAIProviderError>;

    if (error) {
      return [undefined, error];
    }

    // NOTE:(kallebysantos) we need to clone the lambda parser to avoid `this` conflicts inside the local function*
    const parser = this.parse;
    const stream = async function* () {
      for await (const message of generator) {
        // NOTE:(kallebysantos) while streaming the final message will not include 'finish_reason'
        // Instead a '[DONE]' value will be returned to close the stream
        if ("done" in message && message.done) {
          return;
        }

        if ("error" in message) {
          const error = (message.error instanceof Error)
            ? message.error
            : new Error(message.error as string);

          yield [
            undefined,
            {
              inner: {
                error,
                currentValue: null,
              },
              message: "An unknown error was streamed from the provider.",
            } satisfies OpenAIProviderError,
          ];
        }

        yield [parser(message), undefined];

        const finishReason = message.choices.at(0)?.finish_reason;
        if (finishReason && finishReason !== "stop") {
          yield [undefined, {
            inner: {
              error: new Error("Expected a completed response."),
              currentValue: message,
            },
            message:
              `Response could not be completed successfully. Expected 'stop' finish reason got '${finishReason}'`,
          }];
        }
      }

      throw new Error(
        "Did not receive done or success response in stream.",
      );
    };

    return [
      stream() as AsyncIterable<OpenAIProviderOutput>,
      undefined,
    ];
  }

  async getText(
    prompt: OpenAIProviderInput,
    signal: AbortSignal,
  ): Promise<OpenAIProviderOutput> {
    const [generation, generationError] = await this.generate(
      prompt,
      signal,
    ) as Result<OpenAIResponse, OpenAIProviderError>;

    if (generationError) {
      return [undefined, generationError];
    }

    const finishReason = generation.choices[0].finish_reason;

    if (finishReason !== "stop") {
      return [undefined, {
        inner: {
          error: new Error("Expected a completed response."),
          currentValue: generation,
        },
        message:
          `Response could not be completed successfully. Expected 'stop' finish reason got '${finishReason}'`,
      }];
    }

    return [this.parse(generation), undefined];
  }

  private parse(response: OpenAIResponse): ILLMProviderOutput<OpenAIResponse> {
    const { usage } = response;
    const choice = response.choices.at(0);

    return {
      // NOTE:(kallebysantos) while streaming the 'delta'  field will be used instead of 'message'
      value: choice?.message?.content ?? choice?.delta?.content ?? undefined,
      inner: response,
      usage: {
        // NOTE:(kallebysantos) usage maybe 'null' while streaming, but the final message will include it
        inputTokens: usage?.prompt_tokens ?? 0,
        outputTokens: usage?.completion_tokens ?? 0,
        totalTokens: usage?.total_tokens ?? 0,
      },
    };
  }

  private async generate(
    input: OpenAICompatibleInput,
    signal: AbortSignal,
    stream: boolean = false,
  ): Promise<
    Result<AsyncGenerator<OpenAIResponse> | OpenAIResponse, OpenAIProviderError>
  > {
    const res = await fetch(
      new URL("/v1/chat/completions", this.options.baseURL),
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "Authorization": `Bearer ${this.options.apiKey}`,
        },
        body: JSON.stringify(
          {
            ...input,
            model: this.options.model,
            stream,
            stream_options: {
              include_usage: true,
            },
          } satisfies OpenAIRequest,
        ),
        signal,
      },
    );

    // try to extract the json error otherwise return any text content from the response
    if (!res.ok || !res.body) {
      const errorMsg =
        `Failed to fetch inference API host '${this.options.baseURL}'. Status ${res.status}: ${res.statusText}`;

      if (!res.body) {
        const error = {
          inner: new Error("Missing response body."),
          message: errorMsg,
        } satisfies OpenAIProviderError;

        return [undefined, error];
      }

      // safe to extract response body cause it was checked above
      try {
        const error = {
          inner: await res.json(),
          message: errorMsg,
        } satisfies OpenAIProviderError;

        return [undefined, error];
      } catch (_) {
        const error = {
          inner: new Error(await res.text()),
          message: errorMsg,
        } satisfies OpenAIProviderError;

        return [undefined, error];
      }
    }

    if (stream) {
      const stream = parseJSONOverEventStream<OpenAIResponse>(res.body, signal);
      return [stream as AsyncGenerator<OpenAIResponse>, undefined];
    }

    const result: OpenAIResponse = await res.json();
    return [result, undefined];
  }
}
