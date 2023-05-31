use deno_core::error::AnyError;
use deno_core::op;
use deno_core::OpState;
use sb_worker_context::events::WorkerEvents;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;

#[derive(Serialize, Deserialize)]
pub enum RawEvent {
    Event(WorkerEvents),
    Empty,
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
async fn op_event_accept(state: Rc<RefCell<OpState>>) -> Result<RawEvent, AnyError> {
    let mut rx = {
        let mut op_state = state.borrow_mut();
        op_state.take::<mpsc::UnboundedReceiver<WorkerEvents>>()
    };

    let data = rx.try_recv();

    let get_data: Result<RawEvent, AnyError> = match data {
        Ok(event) => Ok(RawEvent::Event(event)),
        Err(err) => match err {
            TryRecvError::Empty => Ok(RawEvent::Empty),
            TryRecvError::Disconnected => Ok(RawEvent::Done),
        },
    };

    let mut op_state = state.borrow_mut();
    op_state.put::<mpsc::UnboundedReceiver<WorkerEvents>>(rx);

    get_data
}

deno_core::extension!(
    sb_user_event_worker,
    ops = [op_event_accept],
    esm = ["event_worker.js"]
);
