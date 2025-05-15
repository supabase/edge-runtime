import {
  assertEquals,
  assertExists,
  assertGreater,
  assertIsError,
  assertLessOrEqual,
  assertStringIncludes,
  assertThrows,
} from "jsr:@std/assert";

const session = new Supabase.ai.Session("gte-small");

assertThrows(() => {
  const _ = new Supabase.ai.Session("gte-small_wrong_name");
}, "invalid 'Session' type");

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
    const [meow, meowError] = await session.run("meow") as [
      number[],
      undefined,
    ];

    // @ts-ignore unkwnow type
    const [love, loveError] = await session.run("I love cats", {
      mean_pool: true,
      normalize: true,
    }) as [number[], undefined];

    // "Valid input should result in ok value"
    {
      assertExists(meow);
      assertExists(love);

      assertEquals(meowError, undefined);
      assertEquals(loveError, undefined);
    }

    // "Invalid input should result in error value"
    {
      const [notCat, notCatError] = await session.run({
        bad_input: { "not a cat": "let fail" },
      }) as [undefined, { message: string; inner: Error }];

      assertEquals(notCat, undefined);

      assertExists(notCatError);
      assertIsError(notCatError.inner);
      assertStringIncludes(
        notCatError.message,
        "must provide a valid prompt value",
      );
    }

    // "Ensures `mean_pool` and `normalize`"
    {
      const sameScore = dotProduct(meow, meow);
      const diffScore = dotProduct(meow, love);

      assertGreater(sameScore, 0.9);
      assertGreater(diffScore, 0.5);
      assertGreater(sameScore, diffScore);

      assertLessOrEqual(sameScore, 1);
      assertLessOrEqual(diffScore, 1);
    }

    return new Response(
      null,
      {
        status: meow instanceof Array && meow.length == 384 ? 200 : 500,
      },
    );
  },
};
