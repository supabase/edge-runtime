const session = new Supabase.ai.Session('llama2');

Deno.serve(async (req: Request) => {
  const params = new URL(req.url).searchParams;
  const prompt = params.get('prompt');
  const output = await session.run(prompt, { stream: true }) as Iterator<any>;
  const body = new ReadableStream({
    async pull(ctrl) {
      const item = await output.next();

      if (item.done) {
        ctrl.close();
        return;
      }

      ctrl.enqueue("data: ");
      ctrl.enqueue(JSON.stringify(item.value));
      ctrl.enqueue("\r\n\r\n");
    }
  });

  return new Response(
    body.pipeThrough(new TextEncoderStream()),
    {
      headers: {
        'Content-Type': 'text/event-stream',
      },
    },
  );
});
