let arr = Deno.readFileSync("./mnt/data/test_cases/meow_20mib.bin") as Uint8Array;

// NOTE: Just defined to prevent the JsRuntime leave from the event loop
Deno.serve(() => {
    console.log(arr.byteLength);
});
