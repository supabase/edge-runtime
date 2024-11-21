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

import { round6 } from '../util.ts';

const wasmBytes = await Deno.readFile(
  new URL(
    'magick.wasm',
    import.meta.resolve('npm:@imagemagick/magick-wasm@0.0.30'),
  ),
);

await initializeImageMagick(
  wasmBytes,
);

// Ensure we do not use browser cache
env.useBrowserCache = false;
env.allowLocalModels = false;

const pipe = await pipeline('image-feature-extraction', 'Xenova/clip-vit-base-patch32', {
  device: 'auto',
});

const preprocessor = (img) => {
  const { width, height } = pipe.processor.feature_extractor.crop_size;

  // We need to resize to fit model dims
  // https://legacy.imagemagick.org/Usage/resize/#space_fill
  img.resize(new MagickGeometry(width, height));
  img.extent(new MagickGeometry(width, height), Gravity.Center, MagickColors.Transparent);

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

Deno.serve(async (req: Request) => {
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

  const output = await pipe(imageInput);
  const snapshot = await req.json();

  if (!snapshot) {
    return Response.json(output.data, { status: 201 });
  }

  assertEquals(output.size, 512);
  assertEquals(output.dims.length, 2);

  // Comparing first 3 predictions

  for (const [idx, expected] of Object.entries(snapshot)) {
    assertAlmostEquals(round6(output.data[idx]), expected);
  }

  return new Response();
});
