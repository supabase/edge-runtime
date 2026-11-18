import EventSourceStream from "ext:ai/util/event_source_stream.mjs";

const core = globalThis.Deno.core;

/**
 * @param {ReadableStream<Uint8Array} itr
 * @param {AbortSignal} signal
 */
const parseJSON = async function* (itr, signal) {
  let buffer = "";

  const decoder = new TextDecoder("utf-8");
  const reader = itr.getReader();

  while (true) {
    try {
      if (signal.aborted) {
        reader.cancel(signal.reason);
        reader.releaseLock();
        return { error: signal.reason };
      }

      const { done, value } = await reader.read();

      if (done) {
        break;
      }

      buffer += decoder.decode(value);

      const parts = buffer.split("\n");

      buffer = parts.pop() ?? "";

      for (const part of parts) {
        yield JSON.parse(part);
      }
    } catch (error) {
      yield { error };
    }
  }

  for (const part of buffer.split("\n").filter((p) => p !== "")) {
    try {
      yield JSON.parse(part);
    } catch (error) {
      yield { error };
    }
  }
};

/**
 * @param {ReadableStream<Uint8Array} itr
 * @param {AbortSignal} signal
 */
const parseJSONOverEventStream = async function* (itr, signal) {
  const decoder = new EventSourceStream();

  itr.pipeThrough(decoder);

  /** @type {ReadableStreamDefaultReader<MessageEvent>} */
  const reader = decoder.readable.getReader();

  while (true) {
    try {
      if (signal.aborted) {
        reader.cancel(signal.reason);
        reader.releaseLock();
        return { error: signal.reason };
      }

      const { done, value } = await reader.read();

      if (done) {
        break;
      }

      yield JSON.parse(value.data);
    } catch (error) {
      yield { error };
    }
  }
};

class Session {
  model;
  /** @deprecated Kept for compatibility; always resolved. */
  init = Promise.resolve();
  is_ext_inference_api;
  inferenceAPIHost;

  constructor(model) {
    this.model = model;
    this.is_ext_inference_api = false;

    // NOTE: `gte-small` used built-in ONNX inference, which was removed. We
    // fail in `run()` rather than here so that a top-level session does not
    // prevent the whole worker from booting.
    if (model === "gte-small") {
      return;
    }

    this.inferenceAPIHost = core.ops.op_get_env("AI_INFERENCE_API_HOST");
    this.is_ext_inference_api = !!this.inferenceAPIHost; // only enable external inference API if env variable is set
  }

  /** @param {string | object} prompt Either a String (ollama) or an OpenAI chat completion body object (openaicompatible): https://platform.openai.com/docs/api-reference/chat/create */
  async run(prompt, opts = {}) {
    if (this.model === "gte-small") {
      throw new Error(
        'Built-in ONNX inference has been removed; Supabase.ai.Session("gte-small") is no longer supported. ' +
          "Generate embeddings with an external inference API instead.",
      );
    }

    if (this.is_ext_inference_api) {
      const stream = opts.stream ?? false;

      // default timeout 60s
      const timeout = typeof opts.timeout === "number" ? opts.timeout : 60;
      const timeoutMs = timeout * 1000;

      /** @type {'ollama' | 'openaicompatible'} */
      const mode = opts.mode ?? "ollama";

      switch (mode) {
        case "ollama":
        case "openaicompatible":
          break;

        default:
          throw new TypeError(`invalid mode: ${mode}`);
      }

      const timeoutSignal = AbortSignal.timeout(timeoutMs);
      const signals = [opts.signal, timeoutSignal]
        .filter((it) => it instanceof AbortSignal);

      const signal = AbortSignal.any(signals);

      const path = mode === "ollama" ? "/api/generate" : "/v1/chat/completions";
      const body = mode === "ollama" ? { prompt } : prompt;

      const res = await fetch(
        new URL(path, this.inferenceAPIHost),
        {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
          },
          body: JSON.stringify({
            model: this.model,
            stream,
            ...body,
          }),
        },
        { signal },
      );

      if (!res.ok) {
        throw new Error(
          `Failed to fetch inference API host. Status ${res.status}: ${res.statusText}`,
        );
      }

      if (!res.body) {
        throw new Error("Missing body");
      }

      const parseGenFn = mode === "ollama"
        ? parseJSON
        : stream === true
        ? parseJSONOverEventStream
        : parseJSON;
      const itr = parseGenFn(res.body, signal);

      if (stream) {
        return (async function* () {
          for await (const message of itr) {
            if ("error" in message) {
              if (message.error instanceof Error) {
                throw message.error;
              } else {
                throw new Error(message.error);
              }
            }

            yield message;

            switch (mode) {
              case "ollama": {
                if (message.done) {
                  return;
                }

                break;
              }

              case "openaicompatible": {
                const finishReason = message.choices[0].finish_reason;

                if (finishReason) {
                  if (finishReason !== "stop") {
                    throw new Error("Expected a completed response.");
                  }

                  return;
                }

                break;
              }

              default:
                throw new Error("unreachable");
            }
          }

          throw new Error(
            "Did not receive done or success response in stream.",
          );
        })();
      } else {
        const message = await itr.next();

        if (message.value && "error" in message.value) {
          const error = message.value.error;

          if (error instanceof Error) {
            throw error;
          } else {
            throw new Error(error);
          }
        }

        const finish = mode === "ollama"
          ? message.value.done
          : message.value.choices[0].finish_reason === "stop";

        if (finish !== true) {
          throw new Error("Expected a completed response.");
        }

        return message.value;
      }
    }

    throw new Error(
      `model not supported: "${this.model}" requires AI_INFERENCE_API_HOST to be set`,
    );
  }
}

const MAIN_WORKER_API = {
  /** @deprecated Built-in ONNX sessions were removed; always resolves to 0. */
  tryCleanupUnusedSession: () => Promise.resolve(0),
};

const USER_WORKER_API = {
  Session,
};

export { MAIN_WORKER_API, USER_WORKER_API };
