use crate::rt_worker::worker_ctx::{create_worker, send_user_worker_request, watch_worker_files};
use anyhow::{anyhow, Error};
use event_worker::events::WorkerEventWithMetadata;
use http::{Request, Response};
use hyper::Body;
use log::error;
use notify::{RecursiveMode, Watcher};
use sb_worker_context::essentials::{
    CreateUserWorkerResult, UserWorkerMsgs, UserWorkerProfile, WorkerContextInitOpts,
    WorkerRuntimeOpts,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot::Sender;
use uuid::Uuid;

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
    pub active_workers: HashMap<String, Uuid>,
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
        &self,
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

        user_worker_rt_opts.service_path = Some(service_path.clone());
        user_worker_rt_opts.key = Some(uuid);

        user_worker_rt_opts.pool_msg_tx = Some(self.worker_pool_msgs_tx.clone());
        user_worker_rt_opts.events_msg_tx = self.worker_event_sender.clone();

        worker_options.conf = WorkerRuntimeOpts::UserWorker(user_worker_rt_opts);
        let worker_pool_msgs_tx = self.worker_pool_msgs_tx.clone();

        tokio::task::spawn(async move {
            let maybe_watch = worker_options.watch.clone();
            let main_path = worker_options.service_path.clone();
            let result = create_worker(worker_options).await;
            match result {
                Ok(worker_request_msg_tx) => {
                    if let Some(watch) = maybe_watch {
                        if watch {
                            watch_worker_files(&main_path);
                        }
                    }

                    let profile = UserWorkerProfile {
                        worker_request_msg_tx,
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
            .insert(profile.service_path.clone(), key);
        self.user_workers.insert(key, profile);
    }

    pub fn send_request(
        &self,
        key: &Uuid,
        req: Request<Body>,
        res_tx: Sender<Result<Response<Body>, Error>>,
    ) {
        let _: Result<(), Error> = match self.user_workers.get(key) {
            Some(worker) => {
                let profile = worker.clone();

                // Create a closure to handle the request and send the response
                let request_handler = async move {
                    let result = send_user_worker_request(profile.worker_request_msg_tx, req).await;
                    match result {
                        Ok(rep) => Ok(rep),
                        Err(err) => {
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
            self.active_workers.remove(&profile.service_path);
        }
    }

    pub fn shutdown(&mut self, key: &Uuid) {
        self.retire(key);
        self.user_workers.remove(key);
    }

    fn maybe_active_worker(&self, service_path: &String, force_create: bool) -> Option<&Uuid> {
        if force_create {
            return None;
        }
        self.active_workers.get(service_path)
    }
}
