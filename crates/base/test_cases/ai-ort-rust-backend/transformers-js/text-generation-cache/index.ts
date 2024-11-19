import { assertGreater } from 'jsr:@std/assert';
import {
  env,
  pipeline,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';

// Browser cache is supported by `deno_cache`
// env.useBrowserCache = true; -> Default config

// Ensure we do not use local models
env.allowLocalModels = false;

// There's a little bug in pipeline that can't resolve the model name by itself
// So we need to explicit pass it
const pipe = await pipeline(
  'text-generation',
  'Xenova/gpt2',
  { device: 'auto', model_file_name: 'decoder_model_merged_quantized' },
);

Deno.serve(async () => {
  const input = 'Once upon a time, there was a';

  const output = await pipe(input, {
    max_new_tokens: 10,
    top_k: 0,
    do_sample: false,
  });

  assertGreater(output[0].generated_text.length, input.length);

  return new Response();
});
