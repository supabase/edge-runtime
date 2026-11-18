import { assertRejects } from "jsr:@std/assert";

// Built-in ONNX inference was removed. Creating a `gte-small` session at the
// top level must not break worker boot; only `run()` should fail.
const session = new Supabase.ai.Session("gte-small");
await session.init;

export default {
  async fetch() {
    await assertRejects(
      () => session.run("meow"),
      Error,
      "Built-in ONNX inference has been removed",
    );

    return new Response(null, { status: 200 });
  },
};
