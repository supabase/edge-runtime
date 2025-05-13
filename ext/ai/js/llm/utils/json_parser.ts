import EventSourceStream from "./event_source_stream.mjs";

// Adapted from https://github.com/ollama/ollama-js/blob/6a4bfe3ab033f611639dfe4249bdd6b9b19c7256/src/utils.ts#L262
// TODO:(kallebysantos) need to simplify it
export async function* parseJSON<T extends object>(
  itr: ReadableStream<Uint8Array>,
  signal: AbortSignal,
) {
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
        yield JSON.parse(part) as T;
      }
    } catch (error) {
      yield { error };
    }
  }

  for (const part of buffer.split("\n").filter((p) => p !== "")) {
    try {
      yield JSON.parse(part) as T;
    } catch (error) {
      yield { error };
    }
  }
}

// TODO:(kallebysantos) need to simplify it
export async function* parseJSONOverEventStream<T extends object>(
  itr: ReadableStream<Uint8Array>,
  signal: AbortSignal,
) {
  const decoder = new EventSourceStream();

  itr.pipeThrough(decoder);

  const reader: ReadableStreamDefaultReader<MessageEvent> = decoder.readable
    .getReader();

  while (true) {
    try {
      if (signal.aborted) {
        reader.cancel(signal.reason);
        reader.releaseLock();
        return { error: signal.reason };
      }

      const { done, value } = await reader.read();

      if (done) {
        yield { done };
        break;
      }

      yield JSON.parse(value.data) as T;
    } catch (error) {
      yield { error };
    }
  }
}
