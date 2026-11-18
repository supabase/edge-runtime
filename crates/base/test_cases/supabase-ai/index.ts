import { assertGreater, assertLessOrEqual } from "jsr:@std/assert";

const session = new Supabase.ai.Session("gte-small");
// Creating a session without running it must not warn.
await new Supabase.ai.Session("gte-small").init;

function dotProduct(a: number[], b: number[]) {
  let result = 0;
  for (let i = 0; i < a.length; i++) {
    result += a[i] * b[i];
  }

  return result;
}

export default {
  async fetch() {
    // Generate embedding
    // @ts-ignore unkwnow type
    const meow: number[] = await session.run("meow", {
      mean_pool: true,
      normalize: true,
    });

    // @ts-ignore unkwnow type
    const love: number[] = await session.run("I love cats", {
      mean_pool: true,
      normalize: true,
    });

    // Ensures `mean_pool` and `normalize`
    const sameScore = dotProduct(meow, meow);
    const diffScore = dotProduct(meow, love);

    assertGreater(sameScore, 0.9);
    assertGreater(diffScore, 0.5);
    assertGreater(sameScore, diffScore);

    assertLessOrEqual(sameScore, 1);
    assertLessOrEqual(diffScore, 1);

    return new Response(
      null,
      {
        status: meow instanceof Array && meow.length == 384 ? 200 : 500,
      },
    );
  },
};
