use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct BootEventPayload {
    pub boot_time: usize,
}

#[derive(Serialize, Deserialize)]
pub struct BootFailureEventPayload {
    msg: String,
}
