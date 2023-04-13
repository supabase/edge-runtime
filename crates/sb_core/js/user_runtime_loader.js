// This file is meant to only have `userRuntimeCleanUp`
// The code should address any user specific runtime behavior
// As well as deletions

function loadUserRuntime() {
    delete globalThis.EdgeRuntime;
}

export { loadUserRuntime };