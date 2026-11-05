// `AI_INFERENCE_API_HOST` points at a server that splits its NDJSON body in the
// middle of a multibyte character.
const session = new Supabase.ai.Session("llama3");

export default {
  async fetch() {
    const { response } = await session.run("hi", { timeout: 10 });

    return Response.json({ response }, {
      status: response === "世界" ? 200 : 500,
    });
  },
};
