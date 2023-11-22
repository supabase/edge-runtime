const core = globalThis.Deno.core;

class SupabaseAI {
	runModel(name, prompt) {
		const result = core.ops.op_sb_ai_run_model(name, prompt);
		return result;
	}
}

export { SupabaseAI };
