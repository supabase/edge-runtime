import os from 'node:os';
import { assertAlmostEquals, assertEquals } from 'jsr:@std/assert';
import {
  env,
  pipeline,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';

import { predicts } from '../zero-shot-classification/predicts.ts';

// Browser cache is supported by `deno_cache`
// env.useBrowserCache = true; -> Default config

// Ensure we do not use local models
env.allowLocalModels = false;

const pipe = await pipeline('zero-shot-classification', null, { device: 'auto' });

Deno.serve(async () => {
  const sequences_to_classify = 'I love making pizza';
  const candidate_labels = ['travel', 'cooking', 'dancing'];

  const output = await pipe(sequences_to_classify, candidate_labels);
  const expectedPredict = predicts[os.arch()]

  assertEquals(output.labels, expectedPredict.labels);

  expectedPredict.scores 
    .map((expected, idx) => {
      assertAlmostEquals(output.scores[idx], expected);
    });

  return new Response();
});
