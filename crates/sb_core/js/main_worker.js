import { SUPABASE_USER_WORKERS } from "ext:sb_user_workers/user_workers.js";

// EdgeRuntime namespace
// FIXME: Make the object read-only
globalThis.EdgeRuntime = {
  userWorkers: SUPABASE_USER_WORKERS
};
