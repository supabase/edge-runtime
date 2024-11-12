import { assertAlmostEquals, assertEquals } from 'jsr:@std/assert';
import {
  env,
  pipeline,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';

// Ensure we do not use browser cache
env.useBrowserCache = false;
env.allowLocalModels = false;

const pipe = await pipeline('fill-mask', null, { device: 'auto' });

Deno.serve(async () => {
  const input = '[MASK] is the capital of England.';

  const output = await pipe(input);

  assertEquals(output[0].token_str, 'london');
  assertEquals(output[0].sequence, 'london is the capital of england.');
  assertAlmostEquals(output[0].score, 0.3513388931751251);

  return new Response();
});
