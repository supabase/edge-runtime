const core = globalThis.Deno.core;

const parseJSON = async function* (itr) {
	const decoder = new TextDecoder('utf-8');
	let buffer = '';

	const reader = itr.getReader();

	while (true) {
		const { done, value: chunk } = await reader.read();

		if (done) {
			break;
		}

		buffer += decoder.decode(chunk);

		const parts = buffer.split('\n');

		buffer = parts.pop() ?? '';

		for (const part of parts) {
			try {
				yield JSON.parse(part);
			} catch (error) {
				console.warn('invalid json: ', part);
			}
		}
	}

	for (const part of buffer.split('\n').filter((p) => p !== '')) {
		try {
			yield JSON.parse(part);
		} catch (error) {
			console.warn('invalid json: ', part);
		}
	}
};

class Session {
	model;
	is_ollama;
	inferenceAPIHost;

	constructor(model) {
		this.model = model;
		this.is_ollama = false;

		if (model === 'gte-small') {
			core.ops.op_sb_ai_init_model(model);
		} else {
			this.inferenceAPIHost = core.ops.op_get_env('AI_INFERENCE_API_HOST');
			this.is_ollama = !!this.inferenceAPIHost; // only enable ollama if env variable is set
		}
	}

	async run(prompt, opts = {}) {
		if (this.is_ollama) {
			const stream = opts.stream ?? false;
			const timeout = opts.timeout ?? 60; // default timeout 60s

			const controller = new AbortController();
			const signal = controller.signal;
			setTimeout(() => controller.abort(new Error('Request timeout')), 30 * 1000);

			const res = await fetch(new URL('/api/generate', this.inferenceAPIHost), {
				body: JSON.stringify({ model: this.model, prompt, stream }),
				method: 'POST',
			}, { signal });

			if (!res.body) {
				throw new Error('Missing body');
			}

			const itr = parseJSON(res.body);

			if (stream) {
				return (async function* () {
					for await (const message of itr) {
						if ('error' in message) {
							throw new Error(message.error);
						}
						yield message;
						if (message.done) {
							return;
						}
					}
					throw new Error('Did not receive done or success response in stream.');
				})();
			} else {
				const message = await itr.next();
				if (!message.value.done && message.value.status !== 'success') {
					throw new Error('Expected a completed response.');
				}
				return message.value;
			}
		}

		const mean_pool = opts.mean_pool ?? true;
		const normalize = opts.normalize ?? true;
		const result = await core.ops.op_sb_ai_run_model(this.model, prompt, mean_pool, normalize);
		return result;
	}
}

export default { Session };
