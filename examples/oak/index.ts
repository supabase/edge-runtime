import { Application, Router } from "https://deno.land/x/oak@v12.3.0/mod.ts";
const router = new Router();
router
  // Note: path will be prefixed with function name
  .get("/oak", (context) => {
    context.response.body =
      "This is an example Oak server running on Edge Functions!";
  })
  .post("/oak/greet", async (context) => {
    // Note: request body will be streamed to the function as chunks, set limit to 0 to fully read it.
    const result = context.request.body({ type: "json", limit: 0 });
    const body = await result.value;
    const name = body.name || "you";

    context.response.body = { msg: `Hey ${name}!` };
  })
  .get("/oak/redirect", (context) => {
    context.response.redirect("https://www.example.com");
  });

const app = new Application();
app.use(router.routes());
app.use(router.allowedMethods());

await app.listen({ port: 8000 });
