use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug)]
pub struct PseudoEvent {}

#[derive(Serialize, Deserialize, Debug)]
pub struct BootEvent {
    pub boot_time: usize,
}
#[derive(Serialize, Deserialize, Debug)]
pub struct BootFailureEvent {
    pub msg: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WorkerMemoryUsed {
    pub total: usize,
    pub heap: usize,
    pub external: usize,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ShutdownReason {
    WallClockTime,
    CPUTime,
    Memory,
    EarlyDrop,
    TerminationRequested,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ShutdownEvent {
    pub reason: ShutdownReason,
    pub cpu_time_used: usize,
    pub memory_used: WorkerMemoryUsed,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UncaughtExceptionEvent {
    pub exception: String,
    pub cpu_time_used: usize,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LogEvent {
    pub msg: String,
    pub level: LogLevel,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum WorkerEvents {
    Boot(BootEvent),
    BootFailure(BootFailureEvent),
    UncaughtException(UncaughtExceptionEvent),
    Shutdown(ShutdownEvent),
    EventLoopCompleted(PseudoEvent),
    Log(LogEvent),
}

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct EventMetadata {
    pub service_path: Option<String>,
    pub execution_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WorkerEventWithMetadata {
    pub event: WorkerEvents,
    pub metadata: EventMetadata,
}

#[derive(Serialize, Deserialize)]
pub enum RawEvent {
    Event(WorkerEventWithMetadata),
    Done,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IncomingEvent {
    event_type: Option<String>,
    data: Option<Vec<u8>>,
    done: bool,
}
