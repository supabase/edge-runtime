const session = new Supabase.ai.Session('gte-small');

export default {
    async fetch() {
        // Generate embedding
        const embedding = await session.run("meow", {
            mean_pool: true,
            normalize: true
        });

        return new Response(
            null,
            {
                status: embedding instanceof Array ? 200 : 500
            }
        );
    }
}