const core = globalThis.Deno.core;

class Session {
	model;

	constructor(model) {
		this.model = model;
		core.ops.op_sb_ai_init_model(model);
	}

	async run(prompt, opts = {}) {
		const mean_pool = opts.mean_pool ?? true;
		const normalize = opts.normalize ?? true;
		const result = await core.ops.op_sb_ai_run_model(this.model, prompt, mean_pool, normalize);
		return result;
	}
}

export default { Session };
