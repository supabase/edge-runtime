import { assertAlmostEquals, assertEquals } from 'jsr:@std/assert';
import {
  env,
  pipeline,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';

// Ensure we do not use browser cache
env.useBrowserCache = false;
env.allowLocalModels = false;

const pipe = await pipeline('token-classification', null, { device: 'auto' });

Deno.serve(async () => {
  const input = "My name is Kalleby and I'm from Brazil.";

  const output = await pipe(input);

  assertEquals(output.length, 3);

  [
    {
      entity: 'B-PER',
      score: 0.9930744171142578,
      index: 4,
      word: 'Kalle',
    },
    {
      entity: 'I-PER',
      score: 0.9974944591522217,
      index: 5,
      word: '##by',
    },
    {
      entity: 'B-LOC',
      score: 0.9998322129249573,
      index: 11,
      word: 'Brazil',
    },
  ].map((expected, idx) => {
    assertEquals(output[idx].entity, expected.entity);
    assertAlmostEquals(output[idx].score, expected.score);
    assertEquals(output[idx].index, expected.index);
    assertEquals(output[idx].word, expected.word);
  });

  return new Response();
});
