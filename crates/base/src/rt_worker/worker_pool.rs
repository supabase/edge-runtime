use crate::rt_worker::worker_ctx::{create_worker, send_user_worker_request, UserWorkerProfile};
use anyhow::{anyhow, bail, Error};
use cityhash::cityhash_1::city_hash_64;
use event_worker::events::WorkerEventWithMetadata;
use http::{Request, Response};
use hyper::Body;
use log::error;
use sb_worker_context::essentials::{
    CreateUserWorkerResult, UserWorkerMsgs, WorkerContextInitOpts, WorkerRuntimeOpts,
};
use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot::Sender;

pub struct WorkerPool {
    pub worker_event_sender: Option<mpsc::UnboundedSender<WorkerEventWithMetadata>>,
    active_user_workers: HashMap<u64, UserWorkerProfile>,
    pub user_worker_msg_tx: mpsc::UnboundedSender<UserWorkerMsgs>,
}

pub enum CreationCodes {
    AlreadyExist,
    Unknown,
}

impl WorkerPool {
    pub(crate) fn new(
        worker_event_sender: Option<UnboundedSender<WorkerEventWithMetadata>>,
        user_worker_msg_tx: mpsc::UnboundedSender<UserWorkerMsgs>,
    ) -> Self {
        Self {
            worker_event_sender,
            active_user_workers: HashMap::new(),
            retired_user_workers: HashMap::new(),
            user_worker_msg_tx,
        }
    }

    pub async fn create_worker(
        &mut self,
        mut worker_options: WorkerContextInitOpts,
        tx: Sender<Result<CreateUserWorkerResult, Error>>,
    ) -> Result<CreationCodes, Error> {
        let mut user_worker_rt_opts = match worker_options.conf {
            WorkerRuntimeOpts::UserWorker(opts) => opts,
            _ => unreachable!(),
        };

        let (key, service_path) = self.derive_worker_key(
            &worker_options.service_path,
            user_worker_rt_opts.force_create,
        );

        if self.worker_already_exists(key, user_worker_rt_opts.force_create) {
            if tx.send(Ok(CreateUserWorkerResult { key })).is_err() {
                bail!("main worker receiver dropped")
            }
            return Ok(CreationCodes::AlreadyExist);
        }

        user_worker_rt_opts.service_path = Some(service_path);
        user_worker_rt_opts.key = Some(key);
        user_worker_rt_opts.execution_id = Some(uuid::Uuid::new_v4());
        user_worker_rt_opts.pool_msg_tx = Some(self.user_worker_msg_tx.clone());
        user_worker_rt_opts.events_msg_tx = self.worker_event_sender.clone();

        worker_options.conf = WorkerRuntimeOpts::UserWorker(user_worker_rt_opts);
        let result = create_worker(worker_options).await;

        match result {
            Ok(user_worker_req_tx) => {
                self.active_user_workers.insert(
                    key,
                    UserWorkerProfile {
                        worker_event_tx: user_worker_req_tx,
                    },
                );
                if tx.send(Ok(CreateUserWorkerResult { key })).is_err() {
                    bail!("main worker receiver dropped")
                };

                Ok(CreationCodes::Unknown)
            }
            Err(e) => {
                if tx.send(Err(e)).is_err() {
                    bail!("main worker receiver dropped")
                } else {
                    bail!("An error has occured")
                }
            }
        }
    }

    pub fn send_request(
        &self,
        key: u64,
        req: Request<Body>,
        tx: Sender<Result<Response<Body>, Error>>,
    ) {
        let _: Result<(), Error> = match self.active_user_workers.get(&key) {
            Some(worker) => {
                let profile = worker.clone();

                // Create a closure to handle the request and send the response
                let request_handler = async move {
                    let req = send_user_worker_request(profile.worker_event_tx, req).await;
                    match req {
                        Ok(rep) => Ok(rep),
                        Err(err) => {
                            error!("failed to send request to user worker: {}", err.to_string());
                            Err(err)
                        }
                    }
                };

                // Spawn the closure as an async task
                tokio::task::spawn(async move {
                    if tx.send(request_handler.await).is_err() {
                        error!("main worker receiver dropped")
                    }
                });

                Ok(())
            }
            None => {
                if tx.send(Err(anyhow!("user worker not available"))).is_err() {
                    error!("main worker receiver dropped")
                }

                Err(anyhow!("user worker not available"))
            }
        };
    }

    pub fn retire(&mut self, key: u64) {
        self.active_user_workers.remove(&key);
    }

    pub fn shutdown(&mut self, key: u64) {
        // check if the worker is already in retired pool
        self.active_user_workers.remove(&key);
    }

    fn derive_worker_key(&self, service_path: &Path, force_create: bool) -> (u64, String) {
        let mut key_input = service_path.to_str().unwrap_or("").to_string();
        if force_create {
            let cur_epoch_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            key_input = format!("{}-{}", key_input, cur_epoch_time.as_millis());
        }

        (city_hash_64(key_input.as_bytes()), key_input)
    }

    fn worker_already_exists(&self, key: u64, force_create: bool) -> bool {
        !force_create && self.active_user_workers.contains_key(&key)
    }
}
