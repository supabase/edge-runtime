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
    gid: () => ops.op_gid(),
    uid: () => ops.op_uid(),
    osUptime: () => ops.op_os_uptime(),
    osRelease: () => ops.op_os_release(),
    loadAvg: () => ops.op_loadavg(),
    hostname: () => ops.op_hostname(),
    systemMemoryInfo: () => ops.op_system_memory_info(),
    consoleSize: () => ({ columns: 80, rows: 24}),
    command: DenoCommand
}

export { osCalls };