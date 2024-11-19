import os from 'node:os';
import { assertAlmostEquals, assertEquals } from 'jsr:@std/assert';
import {
  env,
  pipeline,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';
import { predicts } from '../text-classification/predicts.ts';


// Browser cache is supported by `deno_cache`
// env.useBrowserCache = true; -> Default config

// Ensure we do not use local models
env.allowLocalModels = false;

const pipe = await pipeline('text-classification', null, { device: 'auto' });

Deno.serve(async () => {
  const input = [
    'I love supabase',
    'I hated the movie',
  ];

  const output = await pipe(input);

  assertEquals(output.length, 2);

  predicts[os.arch()]
  .map((expected, idx) => {
    assertEquals(output[idx].label, expected.label);
    assertAlmostEquals(output[idx].score, expected.score);
  });

  return new Response();
});
