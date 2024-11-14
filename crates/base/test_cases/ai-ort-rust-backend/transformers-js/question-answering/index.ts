import os from 'node:os';
import { assertAlmostEquals, assertEquals } from 'jsr:@std/assert';
import {
  env,
  pipeline,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';

import { predicts } from './predicts.ts';

// Ensure we do not use browser cache
env.useBrowserCache = false;
env.allowLocalModels = false;

const pipe = await pipeline('question-answering', null, { device: 'auto' });

Deno.serve(async () => {
  const input = 'Who was Jim Henson?';
  const context = 'Jim Henson was a nice puppet.';

  const output = await pipe(input, context);

  const expectedPredict = predicts[os.arch()];

  assertEquals(output.answer, expectedPredict.answer);
  assertAlmostEquals(output.score,expectedPredict.score);

  return new Response();
});
