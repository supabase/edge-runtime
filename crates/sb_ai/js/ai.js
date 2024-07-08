
import EventSourceStream from 'ext:sb_ai/js/util/event_source_stream.mjs';

const core = globalThis.Deno.core;

const parseJSON = async function* (itr) {
    let buffer = '';

    const decoder = new TextDecoder('utf-8');
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

const parseJSONOverEventStream = async function* (itr) {
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

            /** @type {'ollama' | 'openaicompatible'} */
            const mode = opts.mode ?? 'ollama';

            switch (mode) {
                case 'ollama':
                case 'openaicompatible':
                    break;

                default:
                    throw new TypeError(`invalid mode: ${mode}`);
            }

            const controller = new AbortController();
            const signal = controller.signal;
            setTimeout(
                () => controller.abort(new Error('Request timeout')),
                timeout * 1000
            );

            const path = mode === 'ollama' ? '/api/generate' : '/v1/chat/completions';
            const body = mode === 'ollama' ? { prompt } : {
                messages: [{
                    role: 'user',
                    content: prompt
                }]
            };

            const res = await fetch(
                new URL(path, this.inferenceAPIHost),
                {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'application/json',
                    },
                    body: JSON.stringify({
                        model: this.model,
                        stream,
                        ...body
                    }),
                },
                { signal }
            );

            if (!res.body) {
                throw new Error('Missing body');
            }

            const parseGenFn = mode === 'ollama' ? parseJSON : stream === true ? parseJSONOverEventStream : parseJSON;
            const itr = parseGenFn(res.body);

            if (stream) {
                return (async function* () {
                    for await (const message of itr) {
                        if ('error' in message) {
                            throw new Error(message.error);
                        }

                        yield message;

                        switch (mode) {
                            case 'ollama': {
                                if (message.done) {
                                    return;
                                }

                                break;
                            }

                            case 'openaicompatible': {
                                const finishReason = message.choices[0].finish_reason;

                                if (!!finishReason) {
                                    if (finishReason !== 'stop') {
                                        throw new Error('Expected a completed response.');
                                    }

                                    return;
                                }

                                break;
                            }

                            default:
                                throw new Error('unreachable');
                        }
                    }

                    throw new Error(
                        'Did not receive done or success response in stream.'
                    );
                })();
            } else {
                const message = await itr.next();
                const finish = mode === 'ollama' ? message.value.done : message.value.choices[0].finish_reason === 'stop';

                if (finish !== true) {
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
