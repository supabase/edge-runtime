"use strict";

((window) => {
  const core = window.Deno.core;
  const ops = core.ops;

  function create(service_path, memory_limit_mb, worker_timeout_ms, no_module_cache, import_map_path, env_vars) {
    return core.opAsync("op_create_user_worker",
        service_path, memory_limit_mb, worker_timeout_ms, no_module_cache, import_map_path, env_vars);
  }

  function serveRequest(key, req) {
    const { method, url, headers, body, bodyUsed } = req;

    const headersArray = Array.from(headers.entries());
    let bodyBuffer = null;
    if (method !== "GET" || method !== "HEAD") {
      if (!bodyUsed) {

      }
    }

    return core.opAsync("op_send_to_user_worker", key, method, url, headersArray);
  }

  const userWorkers = {
    create,
    serveRequest,
  };

  window.__bootstrap.userWorkers = userWorkers;
})(this);
