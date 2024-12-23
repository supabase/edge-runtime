export default {
    async fetch(req: Request) {
        const url = new URL(req.url);
        const bucketName = Deno.env.get("S3FS_TEST_BUCKET_NAME")!;
        const key = url.pathname.split("/").slice(2).join("/");
        const f = await Deno.open(`/s3/${bucketName}/${key}`);

        return new Response(f.readable, { status: 200 });
    }
}