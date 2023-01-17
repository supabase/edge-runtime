"use strict";

((window) => {
  const core = window.Deno.core;
  const ops = core.ops;

  function setEnv(key, value) {
    ops.op_set_env(key, value);
  }

  function getEnv(key) {
    return ops.op_get_env(key) ?? undefined;
  }

  function deleteEnv(key) {
    ops.op_delete_env(key);
  }

  const env = {
    get: getEnv,
    toObject() {
      return ops.op_env();
    },
    set: setEnv,
    has(key) {
      return getEnv(key) !== undefined;
    },
    delete: deleteEnv,
  };

  window.__bootstrap.os = {
    env,
  }
})(this);
