const imports = { imports: { imported_func: (arg) => console.log(arg) } };
const wasmCode = new Uint8Array([
  0, 97, 115, 109, 1, 0, 0, 0, 1, 133, 128, 128, 128, 0, 1, 96, 0, 1, 127,
  3, 130, 128, 128, 128, 0, 1, 0, 4, 132, 128, 128, 128, 0, 1, 112, 0, 0,
  5, 131, 128, 128, 128, 0, 1, 0, 1, 6, 129, 128, 128, 128, 0, 0, 7, 145,
  128, 128, 128, 0, 2, 6, 109, 101, 109, 111, 114, 121, 2, 0, 4, 109, 97,
  105, 110, 0, 0, 10, 138, 128, 128, 128, 0, 1, 132, 128, 128, 128, 0, 0,
  65, 42, 11
]);

const res = new Response(wasmCode, { headers: [["Content-Type", "application/wasm"]] })
const r = await WebAssembly.instantiateStreaming(res, imports)
console.log(r)

Deno.serve(async () => {
  return new Response(
    JSON.stringify("wasm"),
    { headers: { "Content-Type": "application/json", "Connection": "keep-alive" } },
  )
})
