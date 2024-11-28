type DecoratorType = "tc39" | "typescript" | "typescript_with_metadata";

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

interface UserWorkerFetchOptions {
    signal?: AbortSignal;
}

interface UserWorkerCreateOptions {
    servicePath?: string | null;
    envVars?: string[][] | [string, string][] | null;
    noModuleCache?: boolean | null;
    importMapPath?: string | null;

    forceCreate?: boolean | null;
    netAccessDisabled?: boolean | null;
    allowNet?: string[] | null;
    allowRemoteModules?: boolean | null;
    customModuleRoot?: string | null;

    maybeEszip?: Uint8Array | null;
    maybeEntrypoint?: string | null;
    maybeModuleCode?: string | null;

    memoryLimitMb?: number | null;
    lowMemoryMultiplier?: number | null;
    workerTimeoutMs?: number | null;
    cpuTimeSoftLimitMs?: number | null;
    cpuTimeHardLimitMs?: number | null;

    decoratorType?: DecoratorType | null;
    jsxImportSourceConfig?: JsxImportBaseConfig | null;

    s3FsConfig?: S3FsConfig | null;
    tmpFsConfig?: TmpFsConfig | null;

    context?: { [key: string]: unknown } | null;
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
        function tryCleanupUnusedSession(): Promise<void>;
    }

    class UserWorker {
        constructor(key: string);

        fetch(request: Request, options?: UserWorkerFetchOptions): Promise<Response>;
        static create(opts: UserWorkerCreateOptions): Promise<UserWorker>;
    }

    export function waitUntil<T>(promise: Promise<T>): Promise<T>;
    export function getRuntimeMetrics(): Promise<RuntimeMetrics>;
    export function applySupabaseTag(src: Request, dest: Request): void;
    export function systemMemoryInfo(): MemInfo;
    export function raiseSegfault(): void;

    export { UserWorker as userWorkers };
}

declare namespace Deno {
    export namespace errors {
        class WorkerRequestCancelled extends Error { }
    }
}