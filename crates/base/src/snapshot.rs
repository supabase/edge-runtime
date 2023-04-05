use deno_core::Snapshot;

pub static CLI_SNAPSHOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/RUNTIME_SNAPSHOT.bin"));

pub fn snapshot() -> Snapshot {
    let data = CLI_SNAPSHOT;
    Snapshot::Static(data)
}
