use crate::edge_runtime::{EdgeCallResult, EdgeRuntime};
use anyhow::{anyhow, bail, Error};
use hyper::{Body, Request, Response};
use log::error;
use sb_worker_context::essentials::{
    CreateUserWorkerResult, EdgeContextInitOpts, EdgeContextOpts, UserWorkerMsgs,
};
use std::collections::HashMap;
use std::thread;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

#[derive(Debug)]
pub struct WorkerRequestMsg {
    pub req: Request<Body>,
    pub res_tx: oneshot::Sender<Result<Response<Body>, hyper::Error>>,
}

pub async fn create_worker(
    init_opts: EdgeContextInitOpts,
) -> Result<mpsc::UnboundedSender<WorkerRequestMsg>, Error> {
    let service_path = init_opts.service_path.clone();

    if !service_path.exists() {
        bail!("service does not exist {:?}", &service_path)
    }

    let (worker_boot_result_tx, worker_boot_result_rx) = oneshot::channel::<Result<(), Error>>();
    let (unix_stream_tx, unix_stream_rx) = mpsc::unbounded_channel::<UnixStream>();

    let _handle: thread::JoinHandle<Result<(), Error>> = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();

        let result: Result<EdgeCallResult, Error> = local.block_on(&runtime, async {
            let result = EdgeRuntime::new(init_opts);

            match result {
                Err(err) => {
                    let _ = worker_boot_result_tx.send(Err(anyhow!("worker boot error")));
                    bail!(err)
                }
                Ok(worker) => {
                    let _ = worker_boot_result_tx.send(Ok(()));
                    // start the worker
                    worker.run(unix_stream_rx).await
                }
            }
        });

        if result.is_err() {
            error!(
                "worker {:?} returned an error: {:?}",
                service_path,
                result.unwrap_err()
            );
        }

        Ok(())
    });

    // create an async task waiting for a request
    let (worker_req_tx, mut worker_req_rx) = mpsc::unbounded_channel::<WorkerRequestMsg>();

    let worker_req_handle: tokio::task::JoinHandle<Result<(), Error>> =
        tokio::task::spawn(async move {
            let unix_stream_tx = unix_stream_tx.clone();

            while let Some(msg) = worker_req_rx.recv().await {
                // create a unix socket pair
                let (sender_stream, recv_stream) = UnixStream::pair()?;

                let _ = unix_stream_tx.clone().send(recv_stream);

                // send the HTTP request to the worker over Unix stream
                let (mut request_sender, connection) =
                    hyper::client::conn::handshake(sender_stream).await?;

                // spawn a task to poll the connection and drive the HTTP state
                tokio::task::spawn(async move {
                    if let Err(e) = connection.without_shutdown().await {
                        error!("Error in worker connection: {}", e);
                    }
                });
                tokio::task::yield_now().await;

                let result = request_sender.send_request(msg.req).await;
                let _ = msg.res_tx.send(result);
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

pub async fn create_user_worker_pool() -> Result<mpsc::UnboundedSender<UserWorkerMsgs>, Error> {
    let (user_worker_msgs_tx, mut user_worker_msgs_rx) =
        mpsc::unbounded_channel::<UserWorkerMsgs>();

    let user_worker_msgs_tx_clone = user_worker_msgs_tx.clone();
    let _handle: tokio::task::JoinHandle<Result<(), Error>> = tokio::spawn(async move {
        let mut user_workers: HashMap<Uuid, mpsc::UnboundedSender<WorkerRequestMsg>> =
            HashMap::new();

        loop {
            match user_worker_msgs_rx.recv().await {
                None => break,
                Some(UserWorkerMsgs::Create(mut worker_options, tx)) => {
                    let key = Uuid::new_v4();
                    let mut user_worker_rt_opts = match worker_options.conf {
                        EdgeContextOpts::UserWorker(opts) => opts,
                        _ => unreachable!(),
                    };
                    user_worker_rt_opts.key = Some(key);
                    user_worker_rt_opts.pool_msg_tx = Some(user_worker_msgs_tx_clone.clone());
                    worker_options.conf = EdgeContextOpts::UserWorker(user_worker_rt_opts);
                    let result = create_worker(worker_options).await;

                    match result {
                        Ok(user_worker_req_tx) => {
                            user_workers.insert(key, user_worker_req_tx);

                            if tx.send(Ok(CreateUserWorkerResult { key })).is_err() {
                                bail!("main worker receiver dropped")
                            };
                        }
                        Err(e) => {
                            if tx.send(Err(e)).is_err() {
                                bail!("main worker receiver dropped")
                            };
                        }
                    }
                }
                Some(UserWorkerMsgs::SendRequest(key, req, tx)) => {
                    if let Some(worker) = user_workers.get(&key) {
                        let (res_tx, res_rx) =
                            oneshot::channel::<Result<Response<Body>, hyper::Error>>();
                        let msg = WorkerRequestMsg { req, res_tx };

                        // send the message to worker
                        worker.send(msg)?;

                        // wait for the response back from the worker
                        let res = res_rx.await??;

                        // send the response back to the caller
                        if tx.send(Ok(res)).is_err() {
                            bail!("main worker receiver dropped")
                        }
                    } else if tx.send(Err(anyhow!("user worker not available"))).is_err() {
                        bail!("main worker receiver dropped")
                    };
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
