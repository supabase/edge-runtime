let blocklist: string[] = Deno.readTextFileSync(".blocklisted")
    .trim()
    .split("\n");

for (const api of blocklist) {
    console.log(api);
    if (Deno[api] === void 0) {
        continue;
    }

    if (typeof Deno[api] !== "function") {
        throw new Error(`invalid api: ${api}`);
    }

    try {
        Deno[api]();
        throw new Error(`unreachable: ${api}`);
    } catch (ex) {
        if (ex instanceof Deno.errors.PermissionDenied) {
            continue;
        } else if (ex instanceof TypeError) {
            if (ex.message === "called MOCK_FN") {
                continue;
            }
        }
    }

    throw new Error(`invalid api: ${api}`);
}