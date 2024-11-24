import { assertAlmostEquals, assertEquals } from 'jsr:@std/assert';
import {
  env,
  pipeline,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';

import { round6 } from '../util.ts';

// Ensure we do not use browser cache
env.useBrowserCache = false;
env.allowLocalModels = false;

const pipe = await pipeline('token-classification', null, { device: 'auto' });

Deno.serve(async (req: Request) => {
  const input = "My name is Kalleby and I'm from Brazil.";

  const output = await pipe(input);
  const snapshot = await req.json();

  if (!snapshot) {
    return Response.json(output, { status: 201 });
  }

  assertEquals(output.length, 3);

  (snapshot as Array<any>)
    .map((expected, idx) => {
      assertEquals(output[idx].entity, expected.entity);
      assertAlmostEquals(round6(output[idx].score), expected.score);
      assertEquals(output[idx].index, expected.index);
      assertEquals(output[idx].word, expected.word);
    });

  return new Response();
});
