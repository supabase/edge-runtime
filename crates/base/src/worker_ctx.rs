use crate::edge_runtime::{EdgeCallResult, EdgeRuntime};
use crate::utils::send_event_if_event_manager_available;
use anyhow::{anyhow, bail, Error};
use cityhash::cityhash_1_1_1::city_hash_64;
use hyper::{Body, Request, Response};
use log::error;
use sb_worker_context::essentials::{
    CreateUserWorkerResult, EdgeContextInitOpts, EdgeContextOpts, EdgeEventRuntimeOpts,
    UserWorkerMsgs,
};
use sb_worker_context::events::{BootEvent, BootFailure, UncaughtException, WorkerEvents};
use std::collections::HashMap;
use std::path::PathBuf;
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug)]
pub struct WorkerRequestMsg {
    pub req: Request<Body>,
    pub res_tx: oneshot::Sender<Result<Response<Body>, hyper::Error>>,
}

#[derive(Debug, Clone)]
pub struct UserWorkerProfile {
    worker_event_tx: mpsc::UnboundedSender<WorkerRequestMsg>,
    event_manager_tx: Option<mpsc::UnboundedSender<WorkerEvents>>,
}

async fn handle_request(
    unix_stream_tx: mpsc::UnboundedSender<UnixStream>,
    msg: WorkerRequestMsg,
) -> Result<(), Error> {
    // create a unix socket pair
    let (sender_stream, recv_stream) = UnixStream::pair()?;

    let _ = unix_stream_tx.send(recv_stream);

    // send the HTTP request to the worker over Unix stream
    let (mut request_sender, connection) = hyper::client::conn::handshake(sender_stream).await?;

    // spawn a task to poll the connection and drive the HTTP state
    tokio::task::spawn(async move {
        if let Err(e) = connection.without_shutdown().await {
            error!("Error in worker connection: {}", e);
        }
    });
    tokio::task::yield_now().await;

    let result = request_sender.send_request(msg.req).await;
    let _ = msg.res_tx.send(result);

    Ok(())
}

pub async fn create_worker(
    init_opts: EdgeContextInitOpts,
    event_manager_opts: Option<EdgeEventRuntimeOpts>,
) -> Result<mpsc::UnboundedSender<WorkerRequestMsg>, Error> {
    let service_path = init_opts.service_path.clone();

    if !service_path.exists() {
        bail!("service does not exist {:?}", &service_path)
    }

    let (worker_boot_result_tx, worker_boot_result_rx) = oneshot::channel::<Result<(), Error>>();
    let (unix_stream_tx, unix_stream_rx) = mpsc::unbounded_channel::<UnixStream>();

    let (worker_key, pool_msg_tx, event_msg_tx) = match init_opts.conf.clone() {
        EdgeContextOpts::UserWorker(worker_opts) => (
            worker_opts.key,
            worker_opts.pool_msg_tx,
            worker_opts.events_msg_tx,
        ),
        _ => (None, None, None),
    };

    // spawn a thread to run the worker
    let _handle: thread::JoinHandle<Result<(), Error>> = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();

        let result: Result<EdgeCallResult, Error> = local.block_on(&runtime, async {
            match EdgeRuntime::new(init_opts, event_manager_opts).await {
                Err(err) => {
                    let _ = worker_boot_result_tx.send(Err(anyhow!("worker boot error")));
                    bail!(err)
                }
                Ok(worker) => {
                    let _ = worker_boot_result_tx.send(Ok(()));
                    worker.run(unix_stream_rx).await
                }
            }
        });

        if let Err(err) = result {
            send_event_if_event_manager_available(
                event_msg_tx,
                WorkerEvents::UncaughtException(UncaughtException {
                    exception: err.to_string(),
                }),
            );
            error!("worker {:?} returned an error: {:?}", service_path, err);
        }

        // remove the worker from pool
        if let Some(k) = worker_key {
            if let Some(tx) = pool_msg_tx {
                let res = tx.send(UserWorkerMsgs::Shutdown(k));
                if res.is_err() {
                    error!(
                        "failed to send the shutdown signal to user worker pool: {:?}",
                        res.unwrap_err()
                    );
                }
            }
        }

        Ok(())
    });

    // create an async task waiting for requests for worker
    let (worker_req_tx, mut worker_req_rx) = mpsc::unbounded_channel::<WorkerRequestMsg>();

    let worker_req_handle: tokio::task::JoinHandle<Result<(), Error>> =
        tokio::task::spawn(async move {
            let unix_stream_tx = unix_stream_tx.clone();

            while let Some(msg) = worker_req_rx.recv().await {
                if let Err(err) = handle_request(unix_stream_tx.clone(), msg).await {
                    error!("worker failed to handle request: {:?}", err);
                }
            }

            Ok(())
        });

    // wait for worker to be successfully booted
    let worker_boot_result = worker_boot_result_rx.await?;
    match worker_boot_result {
        Err(err) => {
            worker_req_handle.abort();
            bail!(err)
        }
        Ok(_) => Ok(worker_req_tx),
    }
}

async fn send_user_worker_request(
    worker_channel: mpsc::UnboundedSender<WorkerRequestMsg>,
    req: Request<Body>,
) -> Result<Response<Body>, Error> {
    let (res_tx, res_rx) = oneshot::channel::<Result<Response<Body>, hyper::Error>>();
    let msg = WorkerRequestMsg { req, res_tx };

    // send the message to worker
    worker_channel.send(msg)?;

    // wait for the response back from the worker
    let res = res_rx.await??;

    // send the response back to the caller

    Ok(res)
}

pub async fn create_event_worker(
    event_worker_path: PathBuf,
    import_map_path: Option<String>,
    no_module_cache: bool,
) -> Result<mpsc::UnboundedSender<WorkerEvents>, Error> {
    let (event_tx, event_rx) = mpsc::unbounded_channel::<WorkerEvents>();

    let _ = create_worker(
        EdgeContextInitOpts {
            service_path: event_worker_path,
            no_module_cache,
            import_map_path,
            env_vars: std::env::vars().collect(),
            conf: EdgeContextOpts::EventsWorker,
        },
        Some(EdgeEventRuntimeOpts { event_rx }),
    )
    .await?;

    Ok(event_tx)
}

pub async fn create_user_worker_pool(
    worker_event_sender: Option<mpsc::UnboundedSender<WorkerEvents>>,
) -> Result<mpsc::UnboundedSender<UserWorkerMsgs>, Error> {
    let (user_worker_msgs_tx, mut user_worker_msgs_rx) =
        mpsc::unbounded_channel::<UserWorkerMsgs>();

    let user_worker_msgs_tx_clone = user_worker_msgs_tx.clone();
    let _handle: tokio::task::JoinHandle<Result<(), Error>> = tokio::spawn(async move {
        let mut user_workers: HashMap<u64, UserWorkerProfile> = HashMap::new();

        loop {
            match user_worker_msgs_rx.recv().await {
                None => break,
                Some(UserWorkerMsgs::Create(mut worker_options, tx)) => {
                    let mut user_worker_rt_opts = match worker_options.conf {
                        EdgeContextOpts::UserWorker(opts) => opts,
                        _ => unreachable!(),
                    };

                    // derive worker key from service path
                    // if force create is set, add current epoch mili seconds to randomize
                    let service_path = worker_options.service_path.to_str().unwrap_or("");
                    let mut key_input = service_path.to_string();
                    if user_worker_rt_opts.force_create {
                        let cur_epoch_time = SystemTime::now().duration_since(UNIX_EPOCH)?;
                        key_input = format!("{}-{}", key_input, cur_epoch_time.as_millis());
                    }
                    let key = city_hash_64(key_input.as_bytes());

                    // do not recreate the worker if it already exists
                    // unless force_create option is set
                    if !user_worker_rt_opts.force_create {
                        if let Some(_worker) = user_workers.get(&key) {
                            if tx.send(Ok(CreateUserWorkerResult { key })).is_err() {
                                bail!("main worker receiver dropped")
                            }
                            continue;
                        }
                    }

                    user_worker_rt_opts.key = Some(key);
                    user_worker_rt_opts.pool_msg_tx = Some(user_worker_msgs_tx_clone.clone());
                    user_worker_rt_opts.events_msg_tx = worker_event_sender.clone();
                    worker_options.conf = EdgeContextOpts::UserWorker(user_worker_rt_opts);
                    let now = Instant::now();
                    let result = create_worker(worker_options, None).await;
                    let elapsed = now.elapsed().as_secs();

                    let event_manager = worker_event_sender.clone();

                    match result {
                        Ok(user_worker_req_tx) => {
                            send_event_if_event_manager_available(
                                event_manager.clone(),
                                WorkerEvents::Boot(BootEvent {
                                    boot_time: elapsed as usize,
                                }),
                            );

                            user_workers.insert(
                                key,
                                UserWorkerProfile {
                                    worker_event_tx: user_worker_req_tx,
                                    event_manager_tx: event_manager,
                                },
                            );
                            if tx.send(Ok(CreateUserWorkerResult { key })).is_err() {
                                bail!("main worker receiver dropped")
                            };
                        }
                        Err(e) => {
                            send_event_if_event_manager_available(
                                event_manager,
                                WorkerEvents::BootFailure(BootFailure { msg: e.to_string() }),
                            );
                            if tx.send(Err(e)).is_err() {
                                bail!("main worker receiver dropped")
                            };
                        }
                    }
                }
                Some(UserWorkerMsgs::SendRequest(key, req, tx)) => {
                    let result = match user_workers.get(&key) {
                        Some(worker) => {
                            let profile = worker.clone();
                            let req = send_user_worker_request(profile.worker_event_tx, req).await;

                            match req {
                                Ok(rep) => Ok(rep),
                                Err(err) => {
                                    send_event_if_event_manager_available(
                                        profile.event_manager_tx,
                                        WorkerEvents::UncaughtException(UncaughtException {
                                            exception: err.to_string(),
                                        }),
                                    );
                                    Err(err)
                                }
                            }
                        }
                        None => {
                            bail!("user worker not available")
                        }
                    };

                    if tx.send(result).is_err() {
                        bail!("main worker receiver dropped")
                    }
                }
                Some(UserWorkerMsgs::Shutdown(key)) => {
                    user_workers.remove(&key);
                }
            }
        }

        Ok(())
    });

    Ok(user_worker_msgs_tx)
}
