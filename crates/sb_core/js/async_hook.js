import { core, primordials } from 'ext:core/mod.js';

const ops = core.ops;
const {
    Promise
} = primordials;

let COUNTER = 0;
const PROMISES = new Map();

function waitUntil(maybePromise) {
    if (maybePromise instanceof Promise) {
        ops.op_tap_promise_metrics("init");
        PROMISES.set(maybePromise, ++COUNTER);
    }

    return maybePromise;
}

function installPromiseHook() {
    core.setPromiseHooks(
        null,
        null,
        null,
        promise => {
            if (PROMISES.delete(promise)) {
                ops.op_tap_promise_metrics("resolve");
            }
        },
    );
}

export {
    waitUntil,
    installPromiseHook
}