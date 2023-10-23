import { is_even } from "https://deno.land/x/is_even@v1.0/mod.ts"
import isEven from "npm:is-even";

globalThis.isTenEven = isEven(10);
console.log(globalThis.isTenEven);