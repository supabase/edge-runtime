use deno_core::v8;
use serde::Deserialize;
use serde::Serialize;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkerHeapStatistics {
  pub total_heap_size: usize,
  pub total_heap_size_executable: usize,
  pub total_physical_size: usize,
  pub total_available_size: usize,
  pub total_global_handles_size: usize,
  pub used_global_handles_size: usize,
  pub used_heap_size: usize,
  pub malloced_memory: usize,
  pub external_memory: usize,
  pub peak_malloced_memory: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkerHeapStatisticsWithServicePath {
  pub service_path: String,
  pub stats: Option<WorkerHeapStatistics>,
}

impl From<&'_ v8::HeapStatistics> for WorkerHeapStatistics {
  fn from(value: &v8::HeapStatistics) -> Self {
    Self {
      total_heap_size: value.total_heap_size(),
      total_heap_size_executable: value.total_heap_size_executable(),
      total_physical_size: value.total_physical_size(),
      total_available_size: value.total_available_size(),
      total_global_handles_size: value.total_global_handles_size(),
      used_global_handles_size: value.used_global_handles_size(),
      used_heap_size: value.used_heap_size(),
      malloced_memory: value.malloced_memory(),
      external_memory: value.external_memory(),
      peak_malloced_memory: value.peak_malloced_memory(),
    }
  }
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, Copy)]
pub struct MemCheckState {
  pub current: WorkerHeapStatistics,
  pub exceeded: bool,
}

#[cfg(test)]
mod test {
  use super::*;
  use deno_core::serde_json;

  #[test]
  fn worker_heap_statistics_default_is_zeroed() {
    let stats = WorkerHeapStatistics::default();
    assert_eq!(stats.total_heap_size, 0);
    assert_eq!(stats.used_heap_size, 0);
    assert_eq!(stats.external_memory, 0);
    assert_eq!(stats.malloced_memory, 0);
    assert_eq!(stats.peak_malloced_memory, 0);
  }

  #[test]
  fn mem_check_state_default_not_exceeded() {
    let state = MemCheckState::default();
    assert!(!state.exceeded);
  }

  #[test]
  fn worker_heap_statistics_serialization_roundtrip() {
    let stats = WorkerHeapStatistics {
      total_heap_size: 1024,
      total_heap_size_executable: 256,
      total_physical_size: 2048,
      total_available_size: 4096,
      total_global_handles_size: 128,
      used_global_handles_size: 64,
      used_heap_size: 512,
      malloced_memory: 100,
      external_memory: 200,
      peak_malloced_memory: 300,
    };

    let json = serde_json::to_string(&stats).unwrap();
    let deserialized: WorkerHeapStatistics =
      serde_json::from_str(&json).unwrap();

    assert_eq!(stats.total_heap_size, deserialized.total_heap_size);
    assert_eq!(stats.used_heap_size, deserialized.used_heap_size);
    assert_eq!(stats.external_memory, deserialized.external_memory);
  }

  #[test]
  fn worker_heap_statistics_with_service_path_default() {
    let stats = WorkerHeapStatisticsWithServicePath::default();
    assert_eq!(stats.service_path, "");
    assert!(stats.stats.is_none());
  }

  #[test]
  fn mem_check_state_serialization() {
    let state = MemCheckState {
      current: WorkerHeapStatistics {
        used_heap_size: 1000,
        ..Default::default()
      },
      exceeded: true,
    };

    let json = serde_json::to_string(&state).unwrap();
    let deserialized: MemCheckState = serde_json::from_str(&json).unwrap();

    assert!(deserialized.exceeded);
    assert_eq!(deserialized.current.used_heap_size, 1000);
  }
}
