"use strict";

((window) => {
  const { TypeError } = window.__bootstrap.primordials;
  const { readableStreamForRid } = window.__bootstrap.streams;
  const core = window.Deno.core;
  const ops = core.ops;

  class UserWorker {
    constructor(key) {
      this.key = key;
    }

    async fetch(req) {
      const { method, url, headers, body, bodyUsed } = req;

      const headersArray = Array.from(headers.entries());
      const hasBody = !(method === "GET" || method === "HEAD" || bodyUsed);

      const userWorkerReq = {
        method,
        url,
        headers: headersArray,
        hasBody,
      };

      const res = await core.opAsync("op_user_worker_fetch", this.key, userWorkerReq);
      const bodyStream = readableStreamForRid(res.bodyRid);
      return new Response(bodyStream, {
        headers: res.headers,
        status: res.status,
        statusText: res.statusText
      });
    }
  }

  async function create(worker_options) {
    const default_options = {
      memory_limit_mb: 150,
      worker_timeout_ms: 60 * 1000,
      no_module_cache: false,
      import_map_path: null,
      env_vars: []
    }

    const {
      service_path,
      memory_limit_mb,
      worker_timeout_ms,
      no_module_cache,
      import_map_path,
      env_vars
    } = { ...default_options, ...worker_options };

    if (!service_path || service_path === "") {
      throw new TypeError("service path must be defined");
    }

    const key = await core.opAsync("op_user_worker_create",
        service_path, memory_limit_mb, worker_timeout_ms, no_module_cache, import_map_path, env_vars);

    return new UserWorker(key);
  }

  const userWorkers = {
    create,
  };

  window.__bootstrap.userWorkers = userWorkers;
})(this);
