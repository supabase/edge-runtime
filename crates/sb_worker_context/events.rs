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
pub enum WorkerEvents {
    Boot(BootEvent),
    BootFailure(BootFailure),
    UncaughtException(UncaughtException),
    TimeLimit(PseudoEvent),
    MemoryLimit(PseudoEvent),
}
