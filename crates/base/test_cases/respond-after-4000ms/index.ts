Deno.serve(async () => {
  await new Promise((resolve) => setTimeout(resolve, 4000));
  return new Response("meow");
});
