// `AI_INFERENCE_API_HOST` points at a server that accepts the connection and
// never answers, so only the abort signal can end `session.run()`.
const session = new Supabase.ai.Session("llama3");

async function outcome(opts: Record<string, unknown>) {
  let guard: number | undefined;
  const pending = new Promise<string>((resolve) => {
    guard = setTimeout(() => resolve("still pending"), 10 * 1000);
  });

  try {
    return await Promise.race([
      session.run("hi", opts).then(() => "resolved"),
      pending,
    ]);
  } catch (e) {
    return `${e.name}: ${e.message}`;
  } finally {
    clearTimeout(guard);
  }
}

export default {
  async fetch() {
    const timeout = await outcome({ timeout: 1 });

    const controller = new AbortController();
    setTimeout(() => controller.abort(new Error("caller aborted")), 500);
    const signal = await outcome({ timeout: 60, signal: controller.signal });

    const ok = timeout === "TimeoutError: Signal timed out." &&
      signal === "Error: caller aborted";

    return Response.json({ timeout, signal }, { status: ok ? 200 : 500 });
  },
};
