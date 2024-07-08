const session = new Supabase.ai.Session('llama2');

Deno.serve(async (req: Request) => {
  const params = new URL(req.url).searchParams;
  const prompt = params.get('prompt');
  const stream = params.get('stream') === 'true' ?? false;

  let mode = params.get("mode") ?? 'ollama';

  switch (mode) {
    case 'ollama':
    case 'openaicompatible':
      break;

    default: {
      mode = 'ollama';
    }
  }

  if (stream) {
    const output = await session.run(prompt, {
      stream,
      mode
    }) as Iterator<any>;

    const body = new ReadableStream({
      async pull(ctrl) {
        const item = await output.next();

        if (item.done) {
          console.log("done!");
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
  } else {
    const output = await session.run(prompt, {
      stream,
      mode
    });

    console.log("done!");
    return Response.json(output);
  }
});
