import { promisify } from "node:util";
import {
  brotliCompress,
  brotliDecompress,
  brotliDecompressSync,
} from "node:zlib";

Deno.serve(async () => {
  const meow = "meow";

  const meowCompressed = await promisify(brotliCompress)(meow);
  const meowDecompressed = await promisify(brotliDecompress)(meowCompressed);

  // Buffer's toString decodes the bytes; a Uint8Array's prints the numbers
  if (brotliDecompressSync(meowCompressed).toString() !== meow) {
    return new Response("brotliDecompressSync mismatch", { status: 500 });
  }

  return new Response(meowDecompressed.toString(), { status: 200 });
});
