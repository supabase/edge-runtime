const core = globalThis.Deno.core;
const ops = core.ops;

class DenoCommand {
    constructor(command, options) {
        this.command = command;
        this.options = options;
    }

    async output() {
        throw new Error("Spawning subprocesses is not allowed on Supabase Edge Runtime.")
    }

    outputSync() {
        throw new Error("Spawning subprocesses is not allowed on Supabase Edge Runtime.");
    }

    spawn() {
        throw new Error("Spawning subprocesses is not allowed on Supabase Edge Runtime.");
    }
}

const osCalls = {
    gid: () => 1000,
    uid: () => 1000,
    osUptime: () => ops.op_os_uptime(),
    osRelease: () => "0.0.0-00000000-generic",
    loadAvg: () => [0, 0, 0],
    hostname: () => "localhost",
    systemMemoryInfo: () => ({
        total: 0,
        free: 0,
        available: 0,
        buffers: 0,
        cached: 0,
        swapTotal: 0,
        swapFree: 0,
    }),
    consoleSize: () => ({ columns: 80, rows: 24}),
    command: DenoCommand,
    version: {
        deno: "",
        "v8": "",
        typescript: ""
    }
}

export { osCalls };