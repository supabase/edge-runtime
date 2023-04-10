import { serve } from "https://deno.land/std@0.140.0/http/server.ts";

const msg = new TextEncoder().encode("data: hellao\r\n\r\n");

serve(async (_) => {
  let timerId: number | undefined;
  const body = new ReadableStream({
    start(controller) {
      timerId = setInterval(() => {
        controller.enqueue(msg);
      }, 1000);
    },
    cancel() {
      console.log('x');
      if (typeof timerId === "number") {
        clearInterval(timerId);
      }
    },
  });
  return new Response(body, {
    headers: {
      "Content-Type": "text/event-stream",
    },
  });
});
