// NOTE: Just defined to prevent the JsRuntime leave from the event loop
Deno.serve(() => { /* do nothing */ });

let buf_in_ext_mem = Deno.readFileSync("./mnt/data/test_cases/meow_20mib.bin") as Uint8Array;
console.log(buf_in_ext_mem.length); // to prevent optimization