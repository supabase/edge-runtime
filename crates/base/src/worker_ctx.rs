use crate::edge_runtime::EdgeRuntime;
use anyhow::{bail, Error};
use hyper::{Body, Request, Response};
use log::error;
use sb_worker_context::essentials::{CreateUserWorkerResult, EdgeContextInitOpts, UserWorkerMsgs};
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
    conf: EdgeContextInitOpts,
) -> Result<mpsc::UnboundedSender<WorkerRequestMsg>, Error> {
    let service_path = conf.service_path.clone();

    if !service_path.exists() {
        bail!("main function does not exist {:?}", &service_path)
    }

    let (unix_stream_tx, unix_stream_rx) = mpsc::unbounded_channel::<UnixStream>();

    let _handle: thread::JoinHandle<Result<(), Error>> = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();

        let _handle: Result<(), Error> = local.block_on(&runtime, async {
            let worker = EdgeRuntime::new(conf)?;

            // start the worker
            let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
            worker.run(unix_stream_rx, shutdown_tx).await?;

            // wait for shutdown signal
            let _ = shutdown_rx.await;

            Ok(())
        });

        // TODO: handle errors in worker
        Ok(())
    });

    // create an async task waiting for a request
    let (worker_req_tx, mut worker_req_rx) = mpsc::unbounded_channel::<WorkerRequestMsg>();

    let _handle: tokio::task::JoinHandle<Result<(), Error>> = tokio::task::spawn(async move {
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
                    error!("Error in main worker connection: {}", e);
                }
            });
            tokio::task::yield_now().await;

            let result = request_sender.send_request(msg.req).await;
            let _ = msg.res_tx.send(result);
        }

        Ok(())
    });

    Ok(worker_req_tx)
}

pub async fn create_user_worker_pool() -> Result<mpsc::UnboundedSender<UserWorkerMsgs>, Error> {
    let (user_worker_msgs_tx, mut user_worker_msgs_rx) =
        mpsc::unbounded_channel::<UserWorkerMsgs>();

    tokio::spawn(async move {
        let mut user_workers: HashMap<Uuid, mpsc::UnboundedSender<WorkerRequestMsg>> =
            HashMap::new();

        loop {
            match user_worker_msgs_rx.recv().await {
                None => break,
                Some(UserWorkerMsgs::Create(worker_options, tx)) => {
                    let result = create_worker(worker_options).await;

                    match result {
                        Ok(user_worker_req_tx) => {
                            let key = Uuid::new_v4();
                            user_workers.insert(key, user_worker_req_tx);

                            let _ = tx.send(Ok(CreateUserWorkerResult { key }));
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e));
                        }
                    }
                }
                Some(UserWorkerMsgs::SendRequest(key, req, tx)) => {
                    // TODO: handle errors
                    let worker = user_workers.get(&key).unwrap();
                    // TODO: Json format
                    // TODO: Ability to attach hook
                    let (res_tx, res_rx) =
                        oneshot::channel::<Result<Response<Body>, hyper::Error>>();
                    let msg = WorkerRequestMsg { req, res_tx };

                    // send the message to worker
                    let _ = worker.send(msg);

                    // wait for the response back from the worker
                    // TODO: handle response errors
                    let res = res_rx.await.unwrap().unwrap();

                    // send the response back to the caller
                    let _ = tx.send(res);
                }
            }
        }
    });

    Ok(user_worker_msgs_tx)
}
