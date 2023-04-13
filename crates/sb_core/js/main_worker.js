import { SUPABASE_USER_WORKERS } from "ext:sb_user_workers/user_workers.js";

Object.defineProperty(globalThis, "EdgeRuntime", {
  get() {
    return {
      userWorkers: SUPABASE_USER_WORKERS
    }
  },
  configurable: true
});