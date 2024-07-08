
import EventSourceStream from 'ext:sb_ai/js/util/event_source_stream.mjs';

const core = globalThis.Deno.core;

const parseJSON = async function* (itr) {
    const decoder = new EventSourceStream();

    itr.pipeThrough(decoder);

    /** @type {ReadableStreamDefaultReader<MessageEvent>} */
    const reader = decoder.readable.getReader();

    while (true) {
        const { done, value } = await reader.read();

        if (done) {
            break;
        }

        try {
            yield JSON.parse(value.data);
        } catch (error) {
            console.warn('invalid message event: ', value);
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
            setTimeout(
                () => controller.abort(new Error('Request timeout')),
                timeout * 1000
            );

            const res = await fetch(
                new URL('/v1/chat/completions', this.inferenceAPIHost),
                {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'application/json',
                    },
                    body: JSON.stringify({
                        model: this.model,
                        stream,
                        messages: [{ role: 'user', content: prompt }],
                    }),
                },
                { signal }
            );

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
                    throw new Error(
                        'Did not receive done or success response in stream.'
                    );
                })();
            } else {
                const message = await itr.next();

                if (
                    !message.value.done &&
                    message.value.choices[0].finish_reason !== 'stop'
                ) {
                    throw new Error('Expected a completed response.');
                }
                return message.value;
            }
        }

        const mean_pool = opts.mean_pool ?? true;
        const normalize = opts.normalize ?? true;
        const result = await core.ops.op_sb_ai_run_model(
            this.model,
            prompt,
            mean_pool,
            normalize
        );
        return result;
    }
}

export default { Session };
