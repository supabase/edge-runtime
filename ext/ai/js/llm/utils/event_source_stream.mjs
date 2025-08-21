import EventStreamParser from "./event_stream_parser.mjs";
/**
 * A Web stream which handles Server-Sent Events from a binary ReadableStream like you get from the fetch API.
 * Implements the TransformStream interface, and can be used with the Streams API as such.
 */
class EventSourceStream {
  constructor() {
    // Two important things to note here:
    // 1. The SSE spec allows for an optional UTF-8 BOM.
    // 2. We have to use a *streaming* decoder, in case two adjacent data chunks are split up in the middle of a
    // multibyte Unicode character. Trying to parse the two separately would result in data corruption.
    const decoder = new TextDecoderStream("utf-8");
    let parser;
    const sseStream = new TransformStream({
      start(controller) {
        parser = new EventStreamParser((data, eventType, lastEventId) => {
          // NOTE:(kallebysantos) Some providers like OpenAI send '[DONE]'
          // to indicates stream terminates, so we need to check if the SSE contains "[DONE]" and close the stream
          if (typeof data === "string" && data.trim() === "[DONE]") {
            controller.terminate?.(); // If supported
            controller.close?.(); // Fallback
            return;
          }

          controller.enqueue(
            new MessageEvent(eventType, { data, lastEventId }),
          );
        });
      },
      transform(chunk) {
        parser.push(chunk);
      },
    });

    decoder.readable.pipeThrough(sseStream);

    this.readable = sseStream.readable;
    this.writable = decoder.writable;
  }
}
export default EventSourceStream;
