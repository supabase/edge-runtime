const core = globalThis.Deno.core;

class Session {
	model;

	constructor(model) {
		core.ops.op_sb_ai_init_model(model);
		this.model = model;
	}

	run(prompt) {
		const result = core.ops.op_sb_ai_run_model(this.model, prompt);
		return result;
	}
}

export default { Session };
