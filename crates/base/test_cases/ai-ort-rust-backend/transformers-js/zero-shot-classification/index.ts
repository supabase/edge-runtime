import { assertAlmostEquals, assertEquals } from 'jsr:@std/assert';
import {
  env,
  pipeline,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';

// Ensure we do not use browser cache
env.useBrowserCache = false;
env.allowLocalModels = false;

const pipe = await pipeline('zero-shot-classification', null, { device: 'auto' });

Deno.serve(async () => {
  const sequences_to_classify = 'I love making pizza';
  const candidate_labels = ['travel', 'cooking', 'dancing'];

  const output = await pipe(sequences_to_classify, candidate_labels);

  assertEquals(output.labels, ['cooking', 'travel', 'dancing']);

  [0.9991624362472264, 0.0004726026797654259, 0.0003649610730082667]
    .map((expected, idx) => {
      assertAlmostEquals(output.scores[idx], expected);
    });

  return new Response();
});
