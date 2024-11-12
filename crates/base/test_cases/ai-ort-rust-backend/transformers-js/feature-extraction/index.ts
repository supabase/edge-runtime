import { assertEquals, assertAlmostEquals } from 'jsr:@std/assert';
import {
  env,
  pipeline,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';

// Ensure we do not use browser cache
env.useBrowserCache = false;
env.allowLocalModels = false;

const pipe = await pipeline('feature-extraction', 'supabase/gte-small', { device: 'auto' }); // 384 dims model

Deno.serve(async () => {
  const input = [
    'This framework generates embeddings for each input sentence',
    'Sentences are passed as a list of string.',
    'The quick brown fox jumps over the lazy dog.',
  ];

  const output = await pipe(input, { pooling: 'mean', normalize: true });

  assertEquals(output.size, 3 * 384);
  assertEquals(output.dims.length, 2);

  // Comparing first 3 predictions
  [-0.050660304725170135, -0.006694655399769545, 0.003071750048547983]
    .map((expected, idx) => {
      assertAlmostEquals(output.data[idx], expected);
    });

  return new Response();
});
