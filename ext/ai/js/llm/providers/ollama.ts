import { Result } from "../../ai.ts";
import {
  ILLMProvider,
  ILLMProviderError,
  ILLMProviderInput,
  ILLMProviderMeta,
  ILLMProviderOptions,
  ILLMProviderOutput,
} from "../llm_session.ts";
import { parseJSON } from "../utils/json_parser.ts";

export type OllamaProviderOptions = ILLMProviderOptions;
export type OllamaProviderInput = ILLMProviderInput<string>;
export type OllamaProviderOutput = Result<
  ILLMProviderOutput<OllamaMessage>,
  OllamaProviderError
>;
export type OllamaProviderError = ILLMProviderError<object>;

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
  output!: OllamaProviderOutput;
  error!: OllamaProviderError;
  options: OllamaProviderOptions;

  constructor(opts: OllamaProviderOptions) {
    this.options = opts;
  }

  // ref: https://github.com/ollama/ollama-js/blob/6a4bfe3ab033f611639dfe4249bdd6b9b19c7256/src/utils.ts#L26
  async getStream(
    prompt: OllamaProviderInput,
    signal: AbortSignal,
  ): Promise<
    Result<AsyncIterable<OllamaProviderOutput>, OllamaProviderError>
  > {
    const [generator, error] = await this.generate(
      prompt,
      signal,
      true,
    ) as Result<AsyncGenerator<OllamaMessage>, OllamaProviderError>;

    if (error) {
      return [undefined, error];
    }

    // NOTE:(kallebysantos) we need to clone the lambda parser to avoid `this` conflicts inside the local function*
    const parser = this.parse;
    const stream = async function* () {
      for await (const message of generator) {
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
            } satisfies OllamaProviderError,
          ];
        }

        yield [parser(message), undefined];

        if (message.done) {
          return;
        }
      }

      throw new Error(
        "Did not receive done or success response in stream.",
      );
    };

    return [
      stream() as AsyncIterable<OllamaProviderOutput>,
      undefined,
    ];
  }

  async getText(
    prompt: OllamaProviderInput,
    signal: AbortSignal,
  ): Promise<OllamaProviderOutput> {
    const [generation, generationError] = await this.generate(
      prompt,
      signal,
    ) as Result<OllamaMessage, OllamaProviderError>;

    if (generationError) {
      return [undefined, generationError];
    }

    if (!generation?.done) {
      return [undefined, {
        inner: {
          error: new Error("Expected a completed response."),
          currentValue: generation,
        },
        message:
          `Response could not be completed successfully. Expected 'done'`,
      }];
    }

    return [this.parse(generation), undefined];
  }

  private parse(message: OllamaMessage): ILLMProviderOutput<OllamaMessage> {
    const { response, prompt_eval_count, eval_count } = message;

    const inputTokens = isNaN(prompt_eval_count) ? 0 : prompt_eval_count;
    const outputTokens = isNaN(eval_count) ? 0 : eval_count;

    return {
      value: response,
      inner: message,
      usage: {
        inputTokens,
        outputTokens,
        totalTokens: inputTokens + outputTokens,
      },
    };
  }

  private async generate(
    prompt: string,
    signal: AbortSignal,
    stream: boolean = false,
  ): Promise<
    Result<AsyncGenerator<OllamaMessage> | OllamaMessage, OllamaProviderError>
  > {
    const res = await fetch(
      new URL("/api/generate", this.options.baseURL),
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          model: this.options.model,
          stream,
          prompt,
        }),
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
        } satisfies OllamaProviderError;

        return [undefined, error];
      }

      // safe to extract response body cause it was checked above
      try {
        const error = {
          inner: await res.json(),
          message: errorMsg,
        } satisfies OllamaProviderError;

        return [undefined, error];
      } catch (_) {
        const error = {
          inner: new Error(await res.text()),
          message: errorMsg,
        } satisfies OllamaProviderError;

        return [undefined, error];
      }
    }

    if (stream) {
      const stream = parseJSON<OllamaMessage>(res.body, signal);
      return [stream as AsyncGenerator<OllamaMessage>, undefined];
    }

    const result: OllamaMessage = await res.json();
    return [result, undefined];
  }
}
