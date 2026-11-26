await Deno.writeTextFile("/tmp/seek-mode", "abcdef");

export default {
  async fetch() {
    const file = await Deno.open("/tmp/seek-mode");
    const buf = new Uint8Array(2);

    const start = await file.seek(2, Deno.SeekMode.Start);
    await file.read(buf);
    const current = await file.seek(-1, Deno.SeekMode.Current);
    const end = await file.seek(0, Deno.SeekMode.End);

    file.close();

    return Response.json({
      start,
      read: new TextDecoder().decode(buf),
      current,
      end,
    });
  },
};
