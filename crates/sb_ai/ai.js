const core = globalThis.Deno.core;

class Session {
	model;
	is_ollama;

	constructor(model) {
		this.model = model;
		this.is_ollama = false;

		if (model === 'llama2' || model === 'mixtral' || model === 'mistral') {
			this.is_ollama = true;
		} else {
			core.ops.op_sb_ai_init_model(model);
		}
	}

	async run(prompt, opts = {}) {
		if (this.is_ollama) {
			const res = await fetch('https://lakshan-ollama.fly.dev/api/generate', {
				body: JSON.stringify({ model: this.model, prompt, stream: false }),
				method: 'POST',
			});
			const json = await res.json();
			return json.response;
		}

		const mean_pool = opts.mean_pool ?? true;
		const normalize = opts.normalize ?? true;
		const result = await core.ops.op_sb_ai_run_model(this.model, prompt, mean_pool, normalize);
		return result;
	}
}

export default { Session };
