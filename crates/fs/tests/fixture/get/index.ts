export default {
  async fetch(req: Request) {
    const url = new URL(req.url);
    const sync = url.searchParams.get("sync") == "true";
    const bucketName = Deno.env.get("S3FS_TEST_BUCKET_NAME")!;
    const key = url.pathname.split("/").slice(2).join("/");

    if (sync) {
      try {
        return new Response(Deno.readFileSync(`/s3/${bucketName}/${key}`), {
          status: 200,
        });
      } catch (err) {
        console.error(err);
        return new Response(null, { status: 500 });
      }
    } else {
      const f = await Deno.open(`/s3/${bucketName}/${key}`);
      return new Response(f.readable, { status: 200 });
    }
  },
};
