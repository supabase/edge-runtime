use crate::rt_worker::worker_ctx::{create_worker, send_user_worker_request};
use anyhow::{anyhow, Error};
use event_worker::events::WorkerEventWithMetadata;
use http::Request;
use hyper::Body;
use log::error;
use sb_workers::context::{
    CreateUserWorkerResult, SendRequestResult, UserWorkerMsgs, UserWorkerProfile,
    WorkerContextInitOpts, WorkerRuntimeOpts,
};
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot::Sender;
use uuid::Uuid;

// Simple implementation of Round Robin for the Active Workers
#[derive(Default)]
pub struct ActiveWorkerRegistry {
    workers: HashSet<Uuid>,
    next: Option<usize>,
}

impl ActiveWorkerRegistry {
    fn get_next_and_try_advance(&mut self) -> Option<&Uuid> {
        if self.workers.is_empty() {
            let _ = self.next.take();
            return None;
        }

        let len = self.workers.len();
        let idx = self
            .next
            .map(|it| if it + 1 > len { 0 } else { it })
            .unwrap_or(0);

        let _ = self.next.insert(idx + 1);

        return self.workers.iter().nth(idx);
    }
}

// every new worker gets a new UUID (can reuse execution_id)
// user_workers - maintain a hashmap of (uuid - workerProfile (include service path))
// active_workers - hashmap of (service_path - uuid)
// retire removed entry for uuid from active
// shutdown removes uuid from both active and user_workers
// create_worker returns true if an active_worker is available for service_path (force create
// retires current one adds new one)
// send_request is called with UUID
pub struct WorkerPool {
    pub user_workers: HashMap<Uuid, UserWorkerProfile>,
    pub active_workers: HashMap<String, ActiveWorkerRegistry>,
    pub worker_pool_msgs_tx: mpsc::UnboundedSender<UserWorkerMsgs>,

    // TODO: refactor this out of worker pool
    pub worker_event_sender: Option<mpsc::UnboundedSender<WorkerEventWithMetadata>>,
}

impl WorkerPool {
    pub(crate) fn new(
        worker_event_sender: Option<UnboundedSender<WorkerEventWithMetadata>>,
        worker_pool_msgs_tx: mpsc::UnboundedSender<UserWorkerMsgs>,
    ) -> Self {
        Self {
            worker_event_sender,
            user_workers: HashMap::new(),
            active_workers: HashMap::new(),
            worker_pool_msgs_tx,
        }
    }

    pub fn create_user_worker(
        &mut self,
        mut worker_options: WorkerContextInitOpts,
        tx: Sender<Result<CreateUserWorkerResult, Error>>,
    ) {
        let mut user_worker_rt_opts = match worker_options.conf {
            WorkerRuntimeOpts::UserWorker(opts) => opts,
            _ => unreachable!(),
        };

        let service_path = worker_options
            .service_path
            .to_str()
            .unwrap_or("")
            .to_string();

        if let Some(active_worker_uuid) =
            self.maybe_active_worker(&service_path, user_worker_rt_opts.force_create)
        {
            if tx
                .send(Ok(CreateUserWorkerResult {
                    key: *active_worker_uuid,
                }))
                .is_err()
            {
                error!("main worker receiver dropped")
            }
            return;
        }

        let uuid = uuid::Uuid::new_v4();
        let (req_start_timing_tx, req_start_timing_rx) = mpsc::unbounded_channel::<()>();
        let (req_end_timing_tx, req_end_timing_rx) = mpsc::unbounded_channel::<()>();

        user_worker_rt_opts.service_path = Some(service_path.clone());
        user_worker_rt_opts.key = Some(uuid);

        user_worker_rt_opts.pool_msg_tx = Some(self.worker_pool_msgs_tx.clone());
        user_worker_rt_opts.events_msg_tx = self.worker_event_sender.clone();

        worker_options.timing_rx_pair = Some((req_start_timing_rx, req_end_timing_rx));
        worker_options.conf = WorkerRuntimeOpts::UserWorker(user_worker_rt_opts);
        let worker_pool_msgs_tx = self.worker_pool_msgs_tx.clone();

        tokio::task::spawn(async move {
            let result = create_worker(worker_options).await;
            match result {
                Ok(worker_request_msg_tx) => {
                    let profile = UserWorkerProfile {
                        worker_request_msg_tx,
                        timing_tx_pair: (req_start_timing_tx, req_end_timing_tx),
                        service_path,
                    };
                    if worker_pool_msgs_tx
                        .send(UserWorkerMsgs::Created(uuid, profile))
                        .is_err()
                    {
                        error!("user worker msgs receiver dropped")
                    }
                    if tx.send(Ok(CreateUserWorkerResult { key: uuid })).is_err() {
                        error!("main worker receiver dropped")
                    };
                }
                Err(e) => {
                    if tx.send(Err(e)).is_err() {
                        error!("main worker receiver dropped")
                    } else {
                        error!("An error has occured")
                    }
                }
            }
        });
    }

    pub fn add_user_worker(&mut self, key: Uuid, profile: UserWorkerProfile) {
        self.active_workers
            .entry(profile.service_path.clone())
            .or_default()
            .workers
            .insert(key);

        self.user_workers.insert(key, profile);
    }

    pub fn send_request(
        &self,
        key: &Uuid,
        req: Request<Body>,
        res_tx: Sender<Result<SendRequestResult, Error>>,
    ) {
        let _: Result<(), Error> = match self.user_workers.get(key) {
            Some(worker) => {
                let profile = worker.clone();
                let (start_req_tx, end_req_tx) = profile.timing_tx_pair.clone();

                // Create a closure to handle the request and send the response
                let request_handler = async move {
                    let _ = start_req_tx.send(());
                    let result = send_user_worker_request(profile.worker_request_msg_tx, req).await;
                    match result {
                        Ok(rep) => Ok((rep, end_req_tx)),
                        Err(err) => {
                            let _ = end_req_tx.send(());
                            error!("failed to send request to user worker: {}", err.to_string());
                            Err(err)
                        }
                    }
                };

                // Spawn the closure as an async task
                tokio::task::spawn(async move {
                    if res_tx.send(request_handler.await).is_err() {
                        error!("main worker receiver dropped")
                    }
                });

                Ok(())
            }
            None => {
                if res_tx
                    .send(Err(anyhow!("user worker not available")))
                    .is_err()
                {
                    error!("main worker receiver dropped")
                }

                Err(anyhow!("user worker not available"))
            }
        };
    }

    pub fn retire(&mut self, key: &Uuid) {
        if let Some(profile) = self.user_workers.get(key) {
            let registry = self
                .active_workers
                .get_mut(&profile.service_path)
                .expect("registry must be initialized at this point");

            if registry.workers.contains(key) {
                registry.workers.remove(key);
            }
        }
    }

    pub fn shutdown(&mut self, key: &Uuid) {
        self.retire(key);
        self.user_workers.remove(key);
    }

    fn maybe_active_worker(&mut self, service_path: &String, force_create: bool) -> Option<&Uuid> {
        if force_create {
            return None;
        }

        self.active_workers
            .get_mut(service_path)
            .and_then(|it| it.get_next_and_try_advance())
    }
}
