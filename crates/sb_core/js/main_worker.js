import { SUPABASE_USER_WORKERS } from "ext:sb_user_workers/user_workers.js";
import { applyWatcherRid } from "ext:sb_core_main_js/js/http.js";
import { core } from "ext:core/mod.js";

const ops = core.ops;

function applyConnectionWatcher(src, dest) {
	applyWatcherRid(src, dest);
}

Object.defineProperty(globalThis, 'EdgeRuntime', {
	get() {
		return {
			userWorkers: SUPABASE_USER_WORKERS,
			getRuntimeMetrics: () => /* async */ ops.op_runtime_metrics(),
			applyConnectionWatcher,
		};
	},
	configurable: true,
});
