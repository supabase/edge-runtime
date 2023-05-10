// This file is meant to only have `userRuntimeCleanUp`
// The code should address any user specific runtime behavior
// As well as deletions
import { fsVars } from "ext:sb_core_main_js/js/denoOverrides.js"

const deleteDenoApis = (apis) => {
    apis.forEach((key) => {
       delete Deno[key];
    });
}

function loadUserRuntime() {
    deleteDenoApis(Object.keys(fsVars));
    delete globalThis.EdgeRuntime;
}

export { loadUserRuntime };