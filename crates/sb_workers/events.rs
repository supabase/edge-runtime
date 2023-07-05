use anyhow::{bail, Error};
use deno_core::op;
use deno_core::OpState;
use sb_worker_context::events::WorkerEvents;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;
use tokio::sync::mpsc;

#[derive(Serialize, Deserialize)]
pub enum RawEvent {
    Event(WorkerEvents),
    Done,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingEvent {
    event_type: Option<String>,
    data: Option<Vec<u8>>,
    done: bool,
}

#[op]
async fn op_event_accept(state: Rc<RefCell<OpState>>) -> Result<RawEvent, Error> {
    let rx = {
        let mut op_state = state.borrow_mut();
        op_state.try_take::<mpsc::UnboundedReceiver<WorkerEvents>>()
    };
    if rx.is_none() {
        bail!("events worker receiver not available")
    }
    let mut rx = rx.unwrap();

    let data = rx.recv().await;

    let mut op_state = state.borrow_mut();
    op_state.put::<mpsc::UnboundedReceiver<WorkerEvents>>(rx);

    match data {
        Some(event) => Ok(RawEvent::Event(event)),
        None => Ok(RawEvent::Done),
    }
}

deno_core::extension!(
    sb_user_event_worker,
    ops = [op_event_accept],
    esm = ["event_worker.js"]
);
