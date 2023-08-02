use deno_core::op;
use deno_core::v8;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type EnvVars = HashMap<String, String>;

/// TODO: Clean it up from events.rs // Circular dependency
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerMemoryUsage {
    pub heap_total: usize,
    pub heap_used: usize,
    pub external: usize,
}

#[op(v8)]
pub fn op_runtime_memory_usage(scope: &mut v8::HandleScope) -> WorkerMemoryUsage {
    let mut s = v8::HeapStatistics::default();
    scope.get_heap_statistics(&mut s);
    WorkerMemoryUsage {
        heap_total: s.total_heap_size(),
        heap_used: s.used_heap_size(),
        external: s.external_memory(),
    }
}

deno_core::extension!(sb_os, ops = [op_runtime_memory_usage,], esm = ["os.js"]);
