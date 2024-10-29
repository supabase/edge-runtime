const meowFile = await Deno.open("/tmp/meow", { createNew: true, write: true });
const meow = new TextEncoder().encode("meowmeow");

export default {
    async fetch() {
        return Response.json({
            written: await meowFile.write(meow),
            content: await Deno.readTextFile("/tmp/meow"),
        });
    }
}