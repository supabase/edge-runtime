use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct PseudoEvent {}

#[derive(Serialize, Deserialize, Debug)]
pub struct BootEvent {
    pub boot_time: usize,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BootFailure {
    pub msg: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UncaughtException {
    pub exception: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LogEvent {
    pub msg: String,
    pub level: LogLevel,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum WorkerEvents {
    Boot(BootEvent),
    BootFailure(BootFailure),
    UncaughtException(UncaughtException),
    CpuTimeLimit(PseudoEvent),
    WallClockTimeLimit(PseudoEvent),
    MemoryLimit(PseudoEvent),
    EventLoopCompleted(PseudoEvent),
    Log(LogEvent),
}
