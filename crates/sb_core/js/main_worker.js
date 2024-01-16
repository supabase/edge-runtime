import { SUPABASE_USER_WORKERS } from 'ext:sb_user_workers/user_workers.js';
import { applyWatcherRid } from 'ext:sb_core_main_js/js/http.js';

Object.defineProperty(globalThis, 'EdgeRuntime', {
	get() {
		return {
			userWorkers: SUPABASE_USER_WORKERS,
			applyConnectionWatcher: (src, dest) => {
				applyWatcherRid(src, dest);
			}
		};
	},
	configurable: true,
});
