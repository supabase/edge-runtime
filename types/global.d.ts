declare type BeforeunloadReason =
  | "cpu"
  | "memory"
  | "wall_clock"
  | "early_drop"
  | "termination";

declare interface WindowEventMap {
  "load": Event;
  "unload": Event;
  "beforeunload": CustomEvent<BeforeunloadReason>;
  "drain": Event;
}

type DecoratorType = 'tc39' | 'typescript' | 'typescript_with_metadata';

interface JsxImportBaseConfig {
    defaultSpecifier?: string | null;
    module?: string | null;
    baseUrl?: string | null;
}

// TODO(Nyannyacha): These two type defs will be provided later.

// deno-lint-ignore no-explicit-any
type S3FsConfig = any;

// deno-lint-ignore no-explicit-any
type TmpFsConfig = any;

type OtelPropagators = "TraceContext" | "Baggage";
type OtelConsoleConfig = "Ignore" | "Capture" | "Replace";
type OtelConfig = {
  tracing_enabled?: boolean;
  metrics_enabled?: boolean;
  console?: OtelConsoleConfig;
  propagators?: OtelPropagators[];
};

interface UserWorkerFetchOptions {
  signal?: AbortSignal;
}

interface PermissionsOptions {
  allow_all?: boolean | null;
  allow_env?: string[] | null;
  deny_env?: string[] | null;
  allow_net?: string[] | null;
  deny_net?: string[] | null;
  allow_ffi?: string[] | null;
  deny_ffi?: string[] | null;
  allow_read?: string[] | null;
  deny_read?: string[] | null;
  allow_run?: string[] | null;
  deny_run?: string[] | null;
  allow_sys?: string[] | null;
  deny_sys?: string[] | null;
  allow_write?: string[] | null;
  deny_write?: string[] | null;
  allow_import?: string[] | null;
}

interface UserWorkerCreateContext {
  sourceMap?: boolean | null;
  importMapPath?: string | null;
  shouldBootstrapMockFnThrowError?: boolean | null;
  suppressEszipMigrationWarning?: boolean | null;
  useReadSyncFileAPI?: boolean | null;
  supervisor?: {
    requestAbsentTimeoutMs?: number | null;
  };
  otel?: {
    [attribute: string]: string;
  };
}

interface UserWorkerCreateOptions {
  servicePath?: string | null;
  envVars?: string[][] | [string, string][] | null;
  noModuleCache?: boolean | null;

  forceCreate?: boolean | null;
  allowRemoteModules?: boolean | null;
  customModuleRoot?: string | null;
  permissions?: PermissionsOptions | null;

  maybeEszip?: Uint8Array | null;
  maybeEntrypoint?: string | null;
  maybeModuleCode?: string | null;

  memoryLimitMb?: number | null;
  lowMemoryMultiplier?: number | null;
  workerTimeoutMs?: number | null;
  cpuTimeSoftLimitMs?: number | null;
  cpuTimeHardLimitMs?: number | null;
  staticPatterns?: string[] | null;

  s3FsConfig?: S3FsConfig | null;
  tmpFsConfig?: TmpFsConfig | null;
  otelConfig?: OtelConfig | null;

  context?: UserWorkerCreateContext | null;
}

interface HeapStatistics {
  totalHeapSize: number;
  totalHeapSizeExecutable: number;
  totalPhysicalSize: number;
  totalAvailableSize: number;
  totalGlobalHandlesSize: number;
  usedGlobalHandlesSize: number;
  usedHeapSize: number;
  mallocedMemory: number;
  externalMemory: number;
  peakMallocedMemory: number;
}

interface RuntimeMetrics {
  mainWorkerHeapStats: HeapStatistics;
  eventWorkerHeapStats?: HeapStatistics;
}

interface MemInfo {
  total: number;
  free: number;
  available: number;
  buffers: number;
  cached: number;
  swapTotal: number;
  swapFree: number;
}

declare namespace EdgeRuntime {
  export namespace ai {
    function tryCleanupUnusedSession(): Promise<number>;
  }

  class UserWorker {
    constructor(key: string);

    fetch(
      request: Request,
      options?: UserWorkerFetchOptions,
    ): Promise<Response>;

    static create(opts: UserWorkerCreateOptions): Promise<UserWorker>;
    static tryCleanupIdleWorkers(timeoutMs: number): Promise<number>;
  }

  export function scheduleTermination(): void;
  export function waitUntil<T>(promise: Promise<T>): Promise<T>;
  export function getRuntimeMetrics(): Promise<RuntimeMetrics>;
  export function applySupabaseTag(src: Request, dest: Request): void;
  export function systemMemoryInfo(): MemInfo;
  export function raiseSegfault(): void;

  export { UserWorker as userWorkers };
}

declare namespace Supabase {
export namespace ai {
        interface ModelOptions {
            /**
             * Pool embeddings by taking their mean. Applies only for `gte-small` model
             */
            mean_pool?: boolean;

            /**
             * Normalize the embeddings result. Applies only for `gte-small` model
             */
            normalize?: boolean;

            /**
             * Stream response from model. Applies only for LLMs like `mistral` (default: false)
             */
            stream?: boolean;

            /**
             * Automatically abort the request to the model after specified time (in seconds). Applies only for LLMs like `mistral` (default: 60)
             */
            timeout?: number;

            /**
             * Mode for the inference API host. (default: 'ollama')
             */
            mode?: 'ollama' | 'openaicompatible';
            signal?: AbortSignal;
        }

        export type TensorDataTypeMap = {
            float32: Float32Array | number[];
            float64: Float64Array | number[];
            string: string[];
            int8: Int8Array | number[];
            uint8: Uint8Array | number[];
            int16: Int16Array | number[];
            uint16: Uint16Array | number[];
            int32: Int32Array | number[];
            uint32: Uint32Array | number[];
            int64: BigInt64Array | number[];
            uint64: BigUint64Array | number[];
            bool: Uint8Array | number[];
        };

        export type TensorMap = { [key: string]: RawTensor<keyof TensorDataTypeMap> };

        export class Session {
            /**
             * Create a new model session using given model
             */
            constructor(model: string);

            /**
             * Execute the given prompt in model session
             */
            run(
                prompt:
                    | string
                    | Omit<
                        import('openai').OpenAI.Chat.ChatCompletionCreateParams,
                        'model' | 'stream'
                    >,
                modelOptions?: ModelOptions,
            ): unknown;
        }

        /** Provides an user friendly interface for the low level *onnx backend API*.
         * A `RawSession` can execute any *onnx* model, but we only recommend it for `tabular` or *self-made* models, where you need mode control of model execution and pre/pos-processing.
         * Consider a high-level implementation like `@huggingface/transformers.js` for generic tasks like `nlp`, `computer-vision` or `audio`.
         *
         * **Example:**
         * ```typescript
         * const session = await RawSession.fromHuggingFace('Supabase/gte-small');
         * // const session = await RawSession.fromUrl("https://example.com/model.onnx");
         *
         * // Prepare the input tensors
         * const inputs = {
         *   input1: new Tensor("float32", [1.0, 2.0, 3.0], [3]),
         *   input2: new Tensor("float32", [4.0, 5.0, 6.0], [3]),
         * };
         *
         * // Run the model
         * const outputs = await session.run(inputs);
         *
         * console.log(outputs.output1); // Output tensor
         * ```
         */
        export class RawSession {
            /**  The underline session's ID.
             * Session's ID are unique for each loaded model, it means that even if a session is constructed twice its will share the same ID.
             */
            id: string;

            /** A list of all input keys the model expects. */
            inputs: string[];

            /** A list of all output keys the model will result. */
            outputs: string[];

            /** Loads a ONNX model session from source URL.
             * Sessions are loaded once, then will keep warm cross worker's requests
             */
            static fromUrl(source: string | URL): Promise<RawSession>;

            /** Loads a ONNX model session from **HuggingFace** repository.
             * Sessions are loaded once, then will keep warm cross worker's requests
             */
            static fromHuggingFace(repoId: string, opts?: {
                /**
                 * @default 'https://huggingface.co'
                 */
                hostname?: string | URL;
                path?: {
                    /**
                     * @default '{REPO_ID}/resolve/{REVISION}/onnx/{MODEL_FILE}?donwload=true'
                     */
                    template?: string;
                    /**
                     * @default 'main'
                     */
                    revision?: string;
                    /**
                     * @default 'model_quantized.onnx'
                     */
                    modelFile?: string;
                };
            }): Promise<RawSession>;

            /** Loads a ONNX model session from **Storage**.
             * Sessions are loaded once, then will keep warm cross worker's requests
             */
            static fromStorage(repoId: string, opts?: {
                /**
                 * @default 'env SUPABASE_URL'
                 */
                hostname?: string | URL;
                mode?: 'public' | {
                    authorization: string;
                };
            }): Promise<RawSession>;

            /** Run the current session with the given inputs.
             * Use `inputs` and `outputs` properties to know the required inputs and expected results for the model session.
             *
             * @param inputs The input tensors required by the model.
             * @returns The output tensors generated by the model.
             *
             * @example
             * ```typescript
             * const session = await RawSession.fromUrl("https://example.com/model.onnx");
             *
             * // Prepare the input tensors
             * const inputs = {
             *   input1: new Tensor("float32", [1.0, 2.0, 3.0], [3]),
             *   input2: new Tensor("float32", [4.0, 5.0, 6.0], [3]),
             * };
             *
             * // Run the model
             * const outputs = await session.run(inputs);
             *
             * console.log(outputs.output1); // Output tensor
             * ```
             */
            run(inputs: TensorMap): Promise<TensorMap>;
        }

        /** A low level representation of model input/output.
         * Supabase's `Tensor` is totally compatible with `@huggingface/transformers.js`'s `Tensor`. It means that you can use its high-level API to apply some common operations like `sum()`, `min()`, `max()`, `normalize()` etc...
         *
         * **Example: Generating embeddings from scratch**
         * ```typescript
         * import { Tensor as HFTensor } from "@huggingface/transformers.js";
         * const { Tensor, RawSession } = Supabase.ai;
         *
         * const session = await RawSession.fromHuggingFace('Supabase/gte-small');
         *
         * // Example only, in real 'feature-extraction' tensors are given from the tokenizer step.
         * const inputs = {
         *    input_ids: new Tensor('float32', [...], [n, 2]),
         *    attention_mask: new Tensor('float32', [...], [n, 2]),
         *    token_types_ids: new Tensor('float32', [...], [n, 2])
         * };
         *
         * const { last_hidden_state } = await session.run(inputs);
         *
         * // Using `transformers.js` APIs
         * const hfTensor = HFTensor.mean_pooling(last_hidden_state, inputs.attention_mask).normalize();
         *
         * return hfTensor.tolist();
         *
         * ```
         */
        export class RawTensor<T extends keyof TensorDataTypeMap> {
            /**  Type of the tensor. */
            type: T;

            /** The data stored in the tensor. */
            data: TensorDataTypeMap[T];

            /**  Dimensions of the tensor. */
            dims: number[];

            /** The total number of elements in the tensor. */
            size: number;

            constructor(type: T, data: TensorDataTypeMap[T], dims: number[]);

            tryEncodeAudio(sampleRate: number): Promise<ArrayBuffer>;
        }
    }
}

declare namespace Deno {
  export namespace errors {
    class WorkerRequestCancelled extends Error {}
    class WorkerAlreadyRetired extends Error {}
  }
}
