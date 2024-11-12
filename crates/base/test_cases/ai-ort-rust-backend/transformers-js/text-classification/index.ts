
import { assertAlmostEquals, assertEquals } from 'jsr:@std/assert';
import {
  env,
  pipeline,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';

// Ensure we do not use browser cache
env.useBrowserCache = false;
env.allowLocalModels = false;

const pipe = await pipeline('text-classification', null, { device: 'auto' });

Deno.serve(async () => {
  const input = [
    'I love supabase',
    'I hated the movie',
  ];

  const output = await pipe(input);

  assertEquals(output.length, 2);

  [
    { label: 'POSITIVE', score: 0.9987488985061646 },
    { label: 'NEGATIVE', score: 0.9996954202651978 },
  ].map((expected, idx) => {
    assertEquals(output[idx].label, expected.label);
    assertAlmostEquals(output[idx].score, expected.score);
  });

  return new Response();
});
