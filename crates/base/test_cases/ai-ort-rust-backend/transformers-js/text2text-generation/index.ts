import { assertGreater, assertStringIncludes } from 'jsr:@std/assert';
import {
  env,
  pipeline,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';

// Ensure we do not use browser cache
env.useBrowserCache = false;
env.allowLocalModels = false;

const pipe = await pipeline('text2text-generation', null, { device: 'auto' });

Deno.serve(async () => {
  const input = 'Premise:  At my age you will probably have learnt one lesson. ' +
    "Hypothesis:  It's not certain how many lessons you'll learn by your thirties. " +
    'Does the premise entail the hypothesis?';

  const output = await pipe(input, {
    top_k: 0,
    do_sample: false,
  });

  assertGreater(output[0].generated_text.length, 0);
  assertStringIncludes(output[0].generated_text, 'no');

  return new Response();
});
