export default {
    async fetch(req: Request) {
        const url = new URL(req.url);
        const stream = req.body;
        const bucketName = Deno.env.get("S3FS_TEST_BUCKET_NAME")!;
        const key = url.pathname.split("/").slice(2).join("/");

        if (!stream || key === "") {
            return new Response(null, { status: 400 });
        }

        const f = await Deno.create(`/s3/${bucketName}/${key}`);

        await stream.pipeTo(f.writable);
        return new Response(null, { status: 200 });
    }
}