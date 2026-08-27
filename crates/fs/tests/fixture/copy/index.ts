export default {
  async fetch(req: Request) {
    const url = new URL(req.url);
    const bucketName = Deno.env.get("S3FS_TEST_BUCKET_NAME")!;
    const sourceKey = url.pathname.split("/").slice(2).join("/");
    const destinationKey = url.searchParams.get("destination");
    const sync = url.searchParams.get("sync") === "true";

    if (!sourceKey || !destinationKey) {
      return new Response(null, { status: 400 });
    }

    const source = `/s3/${bucketName}/${sourceKey}`;
    const destination = `/s3/${bucketName}/${destinationKey}`;

    if (sync) {
      Deno.copyFileSync(source, destination);
    } else {
      await Deno.copyFile(source, destination);
    }

    return new Response(null, { status: 200 });
  },
};
