import os from 'node:os';
import { assertAlmostEquals, assertEquals } from 'jsr:@std/assert';
import {
  env,
  pipeline,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';
import { predicts } from '../token-classification/predicts.ts';

// Browser cache is supported by `deno_cache`
// env.useBrowserCache = true; -> Default config

// Ensure we do not use local models
env.allowLocalModels = false;

const pipe = await pipeline('token-classification', null, { device: 'auto' });

Deno.serve(async () => {
  const input = "My name is Kalleby and I'm from Brazil.";

  const output = await pipe(input);

  assertEquals(output.length, 3);

  predicts[os.arch()]
    .map((expected, idx) => {
      assertEquals(output[idx].entity, expected.entity);
      assertAlmostEquals(output[idx].score, expected.score);
      assertEquals(output[idx].index, expected.index);
      assertEquals(output[idx].word, expected.word);
  });

  return new Response();
});
