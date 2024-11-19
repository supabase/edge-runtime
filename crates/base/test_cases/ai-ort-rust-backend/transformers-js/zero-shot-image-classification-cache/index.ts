import os from 'node:os';
import { assertAlmostEquals, assertEquals } from 'jsr:@std/assert';
import {
  Gravity,
  ImageMagick,
  initializeImageMagick,
  MagickColors,
  MagickFormat,
  MagickGeometry,
} from 'npm:@imagemagick/magick-wasm@0.0.30';

import {
  env,
  pipeline,
  RawImage,
} from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';
import { predicts } from '../zero-shot-image-classification/predicts.ts';

const wasmBytes = await Deno.readFile(
  new URL(
    'magick.wasm',
    import.meta.resolve('npm:@imagemagick/magick-wasm@0.0.30'),
  ),
);

await initializeImageMagick(
  wasmBytes,
);

// Browser cache is supported by `deno_cache`
// env.useBrowserCache = true; -> Default config

// Ensure we do not use local models
env.allowLocalModels = false;

const pipe = await pipeline('zero-shot-image-classification', 'Xenova/clip-vit-base-patch32', {
  device: 'auto',
});

const preprocessor = (img) => {
  const { shortest_edge } = pipe.processor.feature_extractor.size;

  // We need to resize to fit model dims
  // https://legacy.imagemagick.org/Usage/resize/#space_fill
  img.resize(new MagickGeometry(shortest_edge));
  img.extent(
    new MagickGeometry(shortest_edge),
    Gravity.Center,
    MagickColors.Transparent,
  );

  return img
    .write(
      MagickFormat.Rgba,
      (buffer) => ({
        buffer,
        width: img.width,
        height: img.height,
        channels: img.channels.length,
      }),
    );
};

export async function fetchImage(url: string) {
  const imageRes = await fetch(new URL(url));
  const imageBlob = await imageRes.blob();
  const buffer = await imageBlob.arrayBuffer();

  return new Uint8Array(buffer);
}

Deno.serve(async () => {
  const imageFile = await fetchImage(
    'https://huggingface.co/datasets/huggingface/documentation-images/resolve/main/cats.png',
  );

  // Disable default pre-processor transformations
  pipe.processor.feature_extractor.do_resize = false;
  pipe.processor.feature_extractor.do_center_crop = false;

  const image = ImageMagick.read(imageFile, preprocessor);

  const imageInput = new RawImage(
    image.buffer,
    image.width,
    image.height,
    image.channels,
  );

  const output = await pipe(imageInput, ['bird', 'dog', 'cat', 'airplane']);

  predicts[os.arch()]
    .map((expected, idx) => {
      assertEquals(output[idx].label, expected.label);
      assertAlmostEquals(output[idx].score, expected.score);
  });

  return new Response();
});
