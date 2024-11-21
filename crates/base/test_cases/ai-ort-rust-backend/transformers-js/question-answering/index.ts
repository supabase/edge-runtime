import { assertAlmostEquals, assertEquals } from 'jsr:@std/assert';
import {
  env,
  pipeline,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';

import { round6 } from '../util.ts';

// Ensure we do not use browser cache
env.useBrowserCache = false;
env.allowLocalModels = false;

const pipe = await pipeline('question-answering', null, { device: 'auto' });

Deno.serve(async (req: Request) => {
  const input = 'Who was Jim Henson?';
  const context = 'Jim Henson was a nice puppet.';

  const output = await pipe(input, context);
  const snapshot = await req.json();

  if (!snapshot) {
    return Response.json(output, { status: 201 });
  }

  assertEquals(output.answer, snapshot.answer);
  assertAlmostEquals(round6(output.score), snapshot.score);

  return new Response();
});
