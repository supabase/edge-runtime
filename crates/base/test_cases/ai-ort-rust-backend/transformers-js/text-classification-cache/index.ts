import { assertAlmostEquals, assertEquals } from 'jsr:@std/assert';
import {
  env,
  pipeline,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';

import { round6 } from '../util.ts';

// Browser cache is supported by `deno_cache`
// env.useBrowserCache = true; -> Default config

// Ensure we do not use local models
env.allowLocalModels = false;

const pipe = await pipeline('text-classification', null, { device: 'auto' });

Deno.serve(async (req: Request) => {
  const input = [
    'I love supabase',
    'I hated the movie',
  ];

  const output = await pipe(input);
  const snapshot = await req.json();

  if (!snapshot) {
    return Response.json(output, { status: 201 });
  }

  assertEquals(output.length, 2);

  (snapshot as Array<any>)
    .map((expected, idx) => {
      assertEquals(output[idx].label, expected.label);
      assertAlmostEquals(round6(output[idx].score), expected.score);
    });

  return new Response();
});
