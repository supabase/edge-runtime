import { MAIN_WORKER_API as ai } from 'ext:sb_ai/js/ai.js';
import { SUPABASE_USER_WORKERS as userWorkers } from 'ext:sb_user_workers/user_workers.js';
import { applySupabaseTag } from 'ext:sb_core_main_js/js/http.js';
import { core } from 'ext:core/mod.js';

const ops = core.ops;

Object.defineProperty(globalThis, 'EdgeRuntime', {
	get() {
		return {
			ai,
			userWorkers,
			getRuntimeMetrics: () => /* async */ ops.op_runtime_metrics(),
			applySupabaseTag: (src, dest) => applySupabaseTag(src, dest),
			systemMemoryInfo: () => ops.op_system_memory_info(),
		};
	},
	configurable: true,
});
