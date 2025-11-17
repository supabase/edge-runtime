# Supabase AI module

This crate is part of the Supabase Edge Runtime stack and implements AI related
features for the `Supabase.ai` namespace.

## Model Execution Engine

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="/assets/docs/ai/onnx-backend-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="/assets/docs/ai/onnx-backend.svg">
    <img alt="ONNX Backend illustration" src="/assets/docs/ai/onnx-backend.svg" width="350" style="max-width: 100%;">
  </picture>
</p>

`Supabase.ai` uses [onnxruntime](https://onnxruntime.ai/) as internal model
execution engine, backend by [ort pyke](https://ort.pyke.io/) rust bindings.

The **onnxruntime** API is available from `globalThis` and shares similar specs of [onnxruntime-common](https://github.com/microsoft/onnxruntime/tree/main/js/common).

The available items are:

- `Tensor`: Represent a basic tensor with specified dimensions and data type. - "The AI input/output"
- `InferenceSession`: Represent the inner model session. - "The AI model itself"

<details>
<summary>Usage</summary>

It can be used from the exported `globalThis[Symbol.for("onnxruntime")]` -
but manipulating it directly is not trivial, so in the future you may use the [Inference API #501](https://github.com/supabase/edge-runtime/pull/501) for a more user friendly API.

```typescript
const { InferenceSession, Tensor } = globalThis[Symbol.for("onnxruntime")];

// 'create()' supports an url string buffer or the binary data
const modelUrlBuffer = new TextEncoder().encode("https://huggingface.co/Supabase/gte-small/resolve/main/onnx/model_quantized.onnx");
const session = await InferenceSession.create(modelUrlBuffer);

// Example only, in real 'feature-extraction' tensors must be created from the tokenizer step.
const inputs = {
   input_ids: new Tensor('float32', [1, 2, 3...], [1, 384]),
   attention_mask: new Tensor('float32', [...], [1, 384]),
   token_types_ids: new Tensor('float32', [...], [1, 384])
};

const { last_hidden_state } = await session.run(inputs);
console.log(last_hidden_state);
```

</details>

### Third party libs

Originaly this backend was created to implicit integrate with [transformers.js](https://github.com/huggingface/transformers.js/). This way users can still consuming a high-level lib at same time they benefits of all Supabase's Model Execution Engine features, like model optimization and caching.
For further information please check the [PR #436](https://github.com/supabase/edge-runtime/pull/436) as well the [tests folder](/crates/base/test_cases/ai-ort-rust-backend/transformers-js)

> [!WARNING]
> At this moment users need to explicit target `device: 'auto'` to enable the platform compatibility.

```typescript
import { env, pipeline } from 'https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.0.1';

// Broswer cache is now supported for `onnx` models
env.useBrowserCache = true;
env.allowLocalModels = false;

const pipe = await pipeline('feature-extraction', 'supabase/gte-small', { device: 'auto' });

const output = await pipe("This embed will be generated from rust land", {
  pooling: 'mean',
  normalize: true
});
```

### Self-Hosting

**Caching filepath**:
The `EXT_AI_CACHE_DIR` environment variable can be use to set a custom cache path

**Memory clean up**:
For Self-Hosting users an extra method is available for `main/index.ts` scope and should be used to clean up unused sessions, consider adding it into your main entrypoint file:

```typescript
// cleanup unused sessions every 30s
setInterval(async () => {
    try {
        const cleanupCount = await EdgeRuntime.ai.tryCleanupUnusedSession();
        if (cleanupCount == 0) {
            return;
        }
        console.log('EdgeRuntime.ai.tryCleanupUnusedSession', cleanupCount);
    } catch (e) {
        console.error(e.toString());
    }
}, 30 * 1000);
```

## The `Session` class

Prior versions has [introduced](https://supabase.com/blog/ai-inference-now-available-in-supabase-edge-functions) the `Session` class as alternative to `transformers.js` for *gte-small* model and then was used to provide a [LLM interface](https://supabase.com/docs/guides/functions/ai-models?queryGroups=platform&platform=ollama#using-large-language-models-llm) for Ollama and some other providers.

Since the **Model Execution Engine** was created the `Session` class now can focus on LLM interface while the `Session('gte-small')` is for compatibility purposes only.

> [!WARNING]
> Docs for Session class will end here - There's a open [PR #539](https://github.com/supabase/edge-runtime/pull/539) that may change a lot of things for it.
