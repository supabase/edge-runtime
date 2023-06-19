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
    },
    networkInterfaces: () => [
        {"family":"IPv4","name":"lo","address":"127.0.0.1","netmask":"255.0.0.0","scopeid":null,"cidr":"127.0.0.1/8","mac":"00:00:00:00:00:00"},
        {"family":"IPv4","name":"eth0","address":"10.0.0.10","netmask":"255.255.255.0","scopeid":null,"cidr":"10.0.0.10/24","mac":"00:00:00:00:00:01"}
    ]
}

export { osCalls };