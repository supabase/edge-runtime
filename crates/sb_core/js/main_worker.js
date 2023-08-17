import { SUPABASE_USER_WORKERS } from 'ext:sb_user_workers/user_workers.js';
import Eszip from 'ext:sb_eszip/eszip.js';

Object.defineProperty(globalThis, 'EdgeRuntime', {
	get() {
		return {
			userWorkers: SUPABASE_USER_WORKERS,
			eszip: Eszip,
		};
	},
	configurable: true,
});
