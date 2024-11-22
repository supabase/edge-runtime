export default {
    async fetch(req: Request) {
        const url = new URL(req.url);
        const bucketName = Deno.env.get("S3FS_TEST_BUCKET_NAME")!;
        const recursive = url.searchParams.get("recursive") === "true";
        const key = url.pathname.split("/").slice(2).join("/");

        await Deno.remove(`/s3/${bucketName}/${key}`, { recursive });

        return new Response(null, { status: 200 });
    }
}