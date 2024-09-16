/* Using default: Gte-small */
const pipe = new Supabase.ai.Pipeline('supabase-gte');

/* Using custom model
const pipe = new Supabase.ai.Pipeline(
	'feature-extraction',
	'paraphrase-multilingual-MiniLM-L12-v2',
);
*/

// Using different tasks
// const pipe = new Supabase.ai.Pipeline('sentiment-analysis');

Deno.serve(async (req: Request) => {
  const { input } = await req.json();

  const output = await pipe.run(input);
  return new Response(
    JSON.stringify(
      output,
    ),
    {
      headers: {
        'Content-Type': 'application/json',
        'Connection': 'keep-alive',
      },
    },
  );
});
