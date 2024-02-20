const core = globalThis.Deno.core;

class Session {
	model;

	constructor(model) {
		this.model = model;
		core.ops.op_sb_ai_init_model(model);
	}

	async run(prompt) {
		const result = await core.ops.op_sb_ai_run_model(this.model, prompt);
		return result;
	}
}

export default { Session };
