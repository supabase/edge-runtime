Deno.serve(async (req: Request) => {
    // NOTE: https://github.com/whatwg/fetch/issues/1592
    const form = await req.formData();
    const metadata = {};

    for (const [key, file] of form.entries()) {
        if (file instanceof File) {
            metadata[key] = {
                name: file.name,
                size: file.size,
                type: file.type
            };
        };
    }

    return Response.json(metadata);
});