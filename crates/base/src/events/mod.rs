pub mod events_payload;
use serde::{Deserialize, Serialize};

use crate::events::events_payload::{BootEventPayload, BootFailureEventPayload};
use deno_core::JsRuntime;

#[derive(Serialize, Deserialize)]
pub enum EdgeEventTypes {
    Boot(BootEventPayload),
    BootFailure(BootFailureEventPayload),
}

#[derive(Serialize, Deserialize)]
pub struct EventResponse {
    deployment_id: String,
    timestamp: String,
    event_type: EdgeEventTypes,
    event: String,
    execution_id: String,
    region: String,
    context: Option<deno_core::serde_json::Value>,
}

impl Default for EventResponse {
    fn default() -> Self {
        Self {
            deployment_id: String::new(),
            timestamp: String::new(),
            event_type: EdgeEventTypes::Boot(BootEventPayload { boot_time: 0_usize }),
            event: String::new(),
            execution_id: String::new(),
            region: String::new(),
            context: None,
        }
    }
}

pub fn send_internal_event(
    rt: &mut JsRuntime,
    event_type: String,
    data: EventResponse,
) -> Result<(), anyhow::Error> {
    let script = format!(
        "__internal_events__['{}']({})",
        event_type,
        deno_core::serde_json::to_string(&data).unwrap()
    );

    let call_script = rt.execute_script::<String>("<anon>", script);

    if call_script.is_ok() {
        Ok(())
    } else {
        Err(call_script.err().unwrap())
    }
}
