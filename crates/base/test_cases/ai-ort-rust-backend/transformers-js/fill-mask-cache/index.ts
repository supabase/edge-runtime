import os from 'node:os';
import { assertAlmostEquals, assertEquals } from 'jsr:@std/assert';
import {
  env,
  pipeline,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';

import { predicts } from '../fill-mask/predicts.ts';

// Browser cache is supported by `deno_cache`
// env.useBrowserCache = true; -> Default config

// Ensure we do not use local models
env.allowLocalModels = false;

const pipe = await pipeline('fill-mask', null, { device: 'auto' });

Deno.serve(async () => {
  const input = '[MASK] is the capital of England.';

  const output = await pipe(input);

  const expectedPredict = predicts[os.arch()];

  assertEquals(output[0].token_str, expectedPredict.token_str);
  assertEquals(output[0].sequence, expectedPredict.sequence);
  assertAlmostEquals(output[0].score, expectedPredict.score);

  return new Response();
});
