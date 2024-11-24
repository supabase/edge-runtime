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

const pipe = await pipeline('zero-shot-classification', null, { device: 'auto' });

Deno.serve(async (req: Request) => {
  const sequences_to_classify = 'I love making pizza';
  const candidate_labels = ['travel', 'cooking', 'dancing'];

  const output = await pipe(sequences_to_classify, candidate_labels);
  const snapshot = await req.json();

  if (!snapshot) {
    return Response.json(output, { status: 201 });
  }

  assertEquals(output.labels, snapshot.labels);

  (snapshot.scores as Array<any>)
    .map((expected, idx) => {
      assertAlmostEquals(round6(output.scores[idx]), expected);
    });

  return new Response();
});
