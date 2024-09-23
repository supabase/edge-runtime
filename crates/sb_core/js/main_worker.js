import { SUPABASE_USER_WORKERS } from 'ext:sb_user_workers/user_workers.js';
import { applySupabaseTag } from 'ext:sb_core_main_js/js/http.js';
import { core } from 'ext:core/mod.js';

const ops = core.ops;

Object.defineProperty(globalThis, 'EdgeRuntime', {
	get() {
		return {
			userWorkers: SUPABASE_USER_WORKERS,
			getRuntimeMetrics: () => /* async */ ops.op_runtime_metrics(),
			applySupabaseTag: (src, dest) => applySupabaseTag(src, dest),
			systemMemoryInfo: () => ops.op_system_memory_info(),
			raiseSegfault: () => ops.op_raise_segfault(),
		};
	},
	configurable: true,
});
