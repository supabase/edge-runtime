import {
  ILLMProvider,
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
export type OpenAIProviderOutput = ILLMProviderOutput<OpenAIResponse>;

export class OpenAILLMSession implements ILLMProvider, ILLMProviderMeta {
  input!: OpenAIProviderInput;
  output!: OpenAIProviderOutput;
  options: OpenAIProviderOptions;

  constructor(opts: OpenAIProviderOptions) {
    this.options = opts;
  }

  async getStream(
    prompt: OpenAIProviderInput,
    signal: AbortSignal,
  ): Promise<AsyncIterable<OpenAIProviderOutput>> {
    const generator = await this.generate(
      prompt,
      signal,
      true,
    ) as AsyncGenerator<OpenAIResponse>;

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
          if (message.error instanceof Error) {
            throw message.error;
          }

          throw new Error(message.error as string);
        }

        yield parser(message);

        const finish_reason = message.choices.at(0)?.finish_reason;
        if (finish_reason && finish_reason !== "stop") {
          throw new Error("Expected a completed response.");
        }
      }

      throw new Error(
        "Did not receive done or success response in stream.",
      );
    };

    return stream();
  }

  async getText(
    prompt: OpenAIProviderInput,
    signal: AbortSignal,
  ): Promise<OpenAIProviderOutput> {
    const response = await this.generate(
      prompt,
      signal,
    ) as OpenAIResponse;

    const finishReason = response.choices[0].finish_reason;

    if (finishReason !== "stop") {
      throw new Error("Expected a completed response.");
    }

    return this.parse(response);
  }

  private parse(response: OpenAIResponse): OpenAIProviderOutput {
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
  ) {
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

    if (!res.ok) {
      throw new Error(
        `Failed to fetch inference API host. Status ${res.status}: ${res.statusText}`,
      );
    }

    if (!res.body) {
      throw new Error("Missing body");
    }

    if (stream) {
      return parseJSONOverEventStream<OpenAIResponse>(res.body, signal);
    }

    const result: OpenAIResponse = await res.json();

    return result;
  }
}
