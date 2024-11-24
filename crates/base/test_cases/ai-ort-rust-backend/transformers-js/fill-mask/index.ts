import { assertAlmostEquals, assertEquals } from 'jsr:@std/assert';
import {
  env,
  pipeline,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';

import { round6 } from '../util.ts';

// Ensure we do not use browser cache
env.useBrowserCache = false;
env.allowLocalModels = false;

const pipe = await pipeline('fill-mask', null, { device: 'auto' });

Deno.serve(async (req: Request) => {
  const input = '[MASK] is the capital of England.';
  const output = await pipe(input);
  const snapshot = await req.json();

  if (!snapshot) {
    return Response.json(output[0], { status: 201 });
  }

  assertEquals(output[0].token_str, snapshot.token_str);
  assertEquals(output[0].sequence, snapshot.sequence);
  assertAlmostEquals(round6(output[0].score), snapshot.score);

  return new Response();
});
