import { env, pipeline } from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';

// Ensure we do not use browser cache
env.useBrowserCache = false;
env.allowLocalModels = false;

const pipe = await pipeline('feature-extraction', 'supabase/gte-small', { device: 'auto' });

Deno.serve(async (req: Request) => {
    // const { input } = await req.json();
    const input = 'hello world';

    const output = await pipe(input, { pooling: 'mean', normalize: true });

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
