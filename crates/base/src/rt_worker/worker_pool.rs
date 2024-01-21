use crate::rt_worker::worker_ctx::{create_worker, send_user_worker_request};
use anyhow::{anyhow, Context, Error};
use enum_as_inner::EnumAsInner;
use event_worker::events::WorkerEventWithMetadata;
use http::Request;
use hyper::Body;
use log::error;
use sb_core::conn_sync::ConnSync;
use sb_core::util::sync::AtomicFlag;
use sb_workers::context::{
    CreateUserWorkerResult, SendRequestResult, Timing, TimingStatus, UserWorkerMsgs,
    UserWorkerProfile, WorkerContextInitOpts, WorkerRuntimeOpts,
};
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot::Sender;
use tokio::sync::{mpsc, watch, Notify, OwnedSemaphorePermit, Semaphore, TryAcquireError};
use uuid::Uuid;

use super::worker_ctx::TerminationToken;

#[derive(Debug, Clone, Copy, EnumAsInner)]
pub enum SupervisorPolicy {
    PerWorker,
    PerRequest { oneshot: bool },
}

impl Default for SupervisorPolicy {
    fn default() -> Self {
        Self::PerWorker
    }
}

impl FromStr for SupervisorPolicy {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "per_worker" => Ok(Self::PerWorker),
            "per_request" => Ok(Self::PerRequest { oneshot: false }),
            "oneshot" => Ok(Self::PerRequest { oneshot: true }),
            _ => unreachable!(),
        }
    }
}

impl SupervisorPolicy {
    pub fn oneshot() -> Self {
        Self::PerRequest { oneshot: true }
    }

    pub fn is_oneshot(&self) -> bool {
        matches!(self, Self::PerRequest { oneshot: true })
    }
}

#[derive(Clone)]
pub struct WorkerPoolPolicy {
    supervisor_policy: SupervisorPolicy,
    max_parallelism: usize,
    request_wait_timeout_ms: u64,
}

impl Default for WorkerPoolPolicy {
    fn default() -> Self {
        let available_parallelism = std::thread::available_parallelism()
            .ok()
            .map(|it| it.get())
            .unwrap_or(1);

        Self {
            supervisor_policy: SupervisorPolicy::default(),
            max_parallelism: available_parallelism,
            request_wait_timeout_ms: 10000,
        }
    }
}

impl WorkerPoolPolicy {
    pub fn new(
        supervisor: impl Into<Option<SupervisorPolicy>>,
        max_parallelism: impl Into<Option<usize>>,
        request_wait_timeout_ms: impl Into<Option<u64>>,
    ) -> Self {
        let default = Self::default();

        Self {
            supervisor_policy: supervisor.into().unwrap_or(default.supervisor_policy),
            max_parallelism: max_parallelism.into().unwrap_or(default.max_parallelism),
            request_wait_timeout_ms: request_wait_timeout_ms
                .into()
                .unwrap_or(default.request_wait_timeout_ms),
        }
    }
}

#[derive(Clone, Copy)]
struct WorkerId(Uuid, bool);

impl Eq for WorkerId {}

impl PartialEq for WorkerId {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl std::borrow::Borrow<Uuid> for WorkerId {
    fn borrow(&self) -> &Uuid {
        &self.0
    }
}

impl std::hash::Hash for WorkerId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

// Simple implementation of Round Robin for the Active Workers
pub struct ActiveWorkerRegistry {
    workers: HashSet<WorkerId>,
    next: Option<usize>,
    notify_pair: (flume::Sender<Option<Uuid>>, flume::Receiver<Option<Uuid>>),
    sem: Arc<Semaphore>,
}

impl ActiveWorkerRegistry {
    fn new(max_parallelism: usize) -> Self {
        Self {
            workers: HashSet::default(),
            next: Option::default(),
            notify_pair: flume::unbounded(),
            sem: Arc::new(Semaphore::const_new(max_parallelism)),
        }
    }

    fn mark_used_and_try_advance(&mut self, policy: SupervisorPolicy) -> Option<&Uuid> {
        if self.workers.is_empty() {
            let _ = self.next.take();
            return None;
        }

        let len = self.workers.len();
        let idx = self
            .next
            .map(|it| if it + 1 > len { 0 } else { it })
            .unwrap_or(0);

        match self.workers.iter().nth(idx).cloned() {
            Some(WorkerId(key, true)) => match policy {
                SupervisorPolicy::PerWorker => {
                    self.next = Some(idx + 1);
                    self.workers.get(&key).map(|it| &it.0)
                }

                SupervisorPolicy::PerRequest { .. } => {
                    let key = self
                        .workers
                        .replace(WorkerId(key, false))
                        .and_then(|WorkerId(ref key, _)| self.workers.get(key).map(|it| &it.0));

                    self.next = self.workers.iter().position(|it| it.1);
                    key
                }
            },

            _ => {
                let _ = self.next.take();
                None
            }
        }
    }

    fn mark_idle(&mut self, key: &Uuid, policy: SupervisorPolicy) {
        if let Some(WorkerId(key, mark)) = self.workers.get(key).cloned() {
            if policy.is_per_request() {
                if mark {
                    return;
                }

                let _ = self.workers.replace(WorkerId(key, true));
            }

            let (notify_tx, _) = self.notify_pair.clone();
            let _ = notify_tx.send(Some(key));
        }
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
    pub policy: WorkerPoolPolicy,
    pub user_workers: HashMap<Uuid, UserWorkerProfile>,
    pub active_workers: HashMap<String, ActiveWorkerRegistry>,
    pub worker_pool_msgs_tx: mpsc::UnboundedSender<UserWorkerMsgs>,

    // TODO: refactor this out of worker pool
    pub worker_event_sender: Option<mpsc::UnboundedSender<WorkerEventWithMetadata>>,
}

impl WorkerPool {
    pub(crate) fn new(
        policy: WorkerPoolPolicy,
        worker_event_sender: Option<UnboundedSender<WorkerEventWithMetadata>>,
        worker_pool_msgs_tx: mpsc::UnboundedSender<UserWorkerMsgs>,
    ) -> Self {
        Self {
            policy,
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
        termination_token: Option<TerminationToken>,
    ) {
        let service_path = worker_options
            .service_path
            .to_str()
            .unwrap_or("")
            .to_string();

        let is_oneshot_policy = self.policy.supervisor_policy.is_oneshot();
        let force_create = worker_options
            .conf
            .as_user_worker()
            .map_or(false, |it| !is_oneshot_policy && it.force_create);

        if let Some(ref active_worker_uuid) = self.maybe_active_worker(&service_path, force_create)
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

        enum FlowAfterFence {
            Stop,
            Resend(Sender<Result<CreateUserWorkerResult, Error>>),
            Create(
                Option<OwnedSemaphorePermit>,
                Sender<Result<CreateUserWorkerResult, Error>>,
            ),
        }

        let wait_fence_fut = {
            let registry = self
                .active_workers
                .entry(service_path.clone())
                .or_insert_with(|| ActiveWorkerRegistry::new(self.policy.max_parallelism));

            let sem = registry.sem.clone();
            let (_, notify_rx) = registry.notify_pair.clone();
            let wait_timeout =
                tokio::time::sleep(Duration::from_millis(self.policy.request_wait_timeout_ms));

            async move {
                use FlowAfterFence::*;

                match sem.clone().try_acquire_owned() {
                    Ok(permit) => return Create(Some(permit), tx),
                    Err(TryAcquireError::NoPermits) if force_create => {
                        // NOTE(Nyannyacha): Do we need to consider counting the
                        // permit count (that means it affects maximum
                        // parallelism) if in the force creation mode?
                        return Create(None, tx);
                    }

                    _ => {}
                }

                tokio::pin!(wait_timeout);
                loop {
                    tokio::select! {
                        maybe_key = notify_rx.recv_async() => {
                            match maybe_key {
                                Err(x) => {
                                    if tx.send(Err(anyhow!("worker channel is no longer valid: {}", x))).is_err() {
                                        error!("main worker receiver dropped");
                                    }
                                    return Stop;
                                }

                                Ok(Some(_)) => return Resend(tx),
                                Ok(None) => {
                                    if let Ok(permit) = sem.clone().try_acquire_owned() {
                                        return Create(Some(permit), tx);
                                    }
                                }
                            }
                        },

                        () = &mut wait_timeout => {
                            if tx.send(Err(anyhow!("worker did not respond in time"))).is_err() {
                                error!("main worker receiver dropped");
                            }
                            return Stop;
                        }
                    }
                }
            }
        };

        let worker_pool_msgs_tx = self.worker_pool_msgs_tx.clone();
        let events_msg_tx = self.worker_event_sender.clone();
        let supervisor_policy = self.policy.supervisor_policy;

        drop(tokio::spawn(async move {
            let (permit, tx) = match wait_fence_fut.await {
                FlowAfterFence::Stop => return,
                FlowAfterFence::Resend(tx) => {
                    let WorkerContextInitOpts {
                        service_path,
                        no_module_cache,
                        import_map_path,
                        env_vars,
                        conf,
                        maybe_eszip,
                        maybe_module_code,
                        maybe_entrypoint,
                        ..
                    } = worker_options;

                    if worker_pool_msgs_tx
                        .send(UserWorkerMsgs::Create(
                            WorkerContextInitOpts {
                                service_path,
                                no_module_cache,
                                import_map_path,
                                env_vars,
                                events_rx: None,
                                timing: None,
                                conf,
                                maybe_eszip,
                                maybe_module_code,
                                maybe_entrypoint,
                            },
                            tx,
                        ))
                        .is_err()
                    {
                        error!("main worker receiver dropped");
                    }

                    return;
                }

                FlowAfterFence::Create(permit, tx) => (permit, tx),
            };

            let Ok(mut user_worker_rt_opts) = worker_options.conf.into_user_worker() else {
                return;
            };

            let uuid = uuid::Uuid::new_v4();
            let cancel = Arc::<Notify>::default();
            let (req_start_timing_tx, req_start_timing_rx) =
                mpsc::unbounded_channel::<Arc<Notify>>();

            let status = TimingStatus {
                demand: Arc::new(AtomicUsize::new(0)),
                is_retired: Arc::new(AtomicFlag::default()),
            };

            let (req_end_timing_tx, req_end_timing_rx) = mpsc::unbounded_channel::<()>();

            user_worker_rt_opts.service_path = Some(service_path.clone());
            user_worker_rt_opts.key = Some(uuid);

            user_worker_rt_opts.pool_msg_tx = Some(worker_pool_msgs_tx.clone());
            user_worker_rt_opts.events_msg_tx = events_msg_tx;
            user_worker_rt_opts.cancel = Some(cancel.clone());

            worker_options.timing = Some(Timing {
                status: status.clone(),
                req: (req_start_timing_rx, req_end_timing_rx),
            });

            worker_options.conf = WorkerRuntimeOpts::UserWorker(user_worker_rt_opts);

            match create_worker((worker_options, supervisor_policy, termination_token.clone()))
                .await
            {
                Ok(worker_request_msg_tx) => {
                    let profile = UserWorkerProfile {
                        worker_request_msg_tx,
                        timing_tx_pair: (req_start_timing_tx, req_end_timing_tx),
                        service_path,
                        permit: permit.map(Arc::new),
                        status: status.clone(),
                        cancel,
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

                    status.demand.fetch_add(1, Ordering::Release);
                }
                Err(e) => {
                    if tx.send(Err(e)).is_err() {
                        error!("main worker receiver dropped")
                    } else {
                        error!("An error has occured")
                    }
                }
            }
        }));
    }

    pub fn add_user_worker(&mut self, key: Uuid, profile: UserWorkerProfile) {
        let registry = self
            .active_workers
            .entry(profile.service_path.clone())
            .or_insert_with(|| ActiveWorkerRegistry::new(self.policy.max_parallelism));

        registry
            .workers
            .insert(WorkerId(key, self.policy.supervisor_policy.is_per_worker()));

        self.user_workers.insert(key, profile);
    }

    pub fn send_request(
        &self,
        key: &Uuid,
        req: Request<Body>,
        res_tx: Sender<Result<SendRequestResult, Error>>,
        conn_watch: Option<watch::Receiver<ConnSync>>,
    ) {
        let _: Result<(), Error> = match self.user_workers.get(key) {
            Some(worker) => {
                let policy = self.policy.supervisor_policy;
                let profile = worker.clone();
                let cancel = worker.cancel.clone();
                let (req_start_tx, req_end_tx) = profile.timing_tx_pair.clone();

                // Create a closure to handle the request and send the response
                let request_handler = async move {
                    if !policy.is_per_worker() {
                        let fence = Arc::new(Notify::const_new());

                        if let Err(ex) = req_start_tx.send(fence.clone()) {
                            // NOTE(Nyannyacha): The only way to be trapped in
                            // this branch is if the supervisor associated with
                            // the isolate has been terminated for some reason,
                            // such as a wall-clock timeout.
                            //
                            // It can be expected enough if many isolates are
                            // created at once due to requests rapidly
                            // increasing.
                            //
                            // To prevent this, we must give a wall-clock time
                            // limit enough to each supervisor.
                            error!("failed to notify the fence to the supervisor");
                            return Err(ex)
                                .with_context(|| "failed to notify the fence to the supervisor");
                        }

                        fence.notified().await;
                    }

                    let result = send_user_worker_request(
                        profile.worker_request_msg_tx,
                        cancel,
                        req,
                        conn_watch,
                    )
                    .await;

                    match result {
                        Ok(rep) => Ok((rep, req_end_tx)),
                        Err(err) => {
                            let _ = req_end_tx.send(());
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

    pub fn idle(&mut self, key: &Uuid) {
        if let Some(registry) = self
            .user_workers
            .get_mut(key)
            .and_then(|it| self.active_workers.get_mut(&it.service_path))
        {
            registry.mark_idle(key, self.policy.supervisor_policy);
        }
    }

    pub fn shutdown(&mut self, key: &Uuid) {
        self.retire(key);

        let Some((notify_tx, _)) = self
            .user_workers
            .remove(key)
            .and_then(|it| self.active_workers.get(&it.service_path))
            .map(|it| it.notify_pair.clone())
        else {
            return;
        };

        let _ = notify_tx.send(None);
    }

    fn retire(&mut self, key: &Uuid) {
        if let Some(profile) = self.user_workers.get_mut(key) {
            let registry = self
                .active_workers
                .get_mut(&profile.service_path)
                .expect("registry must be initialized at this point");

            let _ = profile.permit.take();
            let (notify_tx, _) = registry.notify_pair.clone();

            for _ in 0..notify_tx.receiver_count() {
                let _ = notify_tx.send(None);
            }

            if registry.workers.contains(key) {
                registry.workers.remove(key);
            }
        }
    }

    fn maybe_active_worker(&mut self, service_path: &String, force_create: bool) -> Option<Uuid> {
        if force_create {
            return None;
        }

        let Some(registry) = self.active_workers.get_mut(service_path) else {
            return None;
        };

        let policy = self.policy.supervisor_policy;
        let mut advance_fn = move || registry.mark_used_and_try_advance(policy).copied();

        let Some(worker_uuid) = advance_fn() else {
            return None;
        };

        match self
            .user_workers
            .get(&worker_uuid)
            .map(|it| it.status.is_retired.clone())
        {
            Some(is_retired) if !is_retired.is_raised() => {
                self.user_workers
                    .get(&worker_uuid)
                    .map(|it| it.status.demand.as_ref())
                    .unwrap()
                    .fetch_add(1, Ordering::Release);

                Some(worker_uuid)
            }

            _ => {
                self.retire(&worker_uuid);
                self.maybe_active_worker(service_path, force_create)
            }
        }
    }
}
