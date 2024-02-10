import { SUPABASE_USER_WORKERS } from "ext:sb_user_workers/user_workers.js";
import { getterOnly } from "ext:sb_core_main_js/js/fieldUtils.js";
import { applyWatcherRid } from "ext:sb_core_main_js/js/http.js";
import { core } from "ext:core/mod.js";

const ops = core.ops;

Object.defineProperty(globalThis, 'EdgeRuntime', {
	get() {
		return {
			userWorkers: SUPABASE_USER_WORKERS,
			runtimeMetrics: getterOnly(() => ops.op_runtime_metrics()),
			applyConnectionWatcher: (src, dest) => {
				applyWatcherRid(src, dest);
			}
		};
	},
	configurable: true,
});
