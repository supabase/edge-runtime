use crate::utils::units::{bytes_to_display, human_elapsed, mib_to_bytes};

use anyhow::Error;
use deno_core::url::Url;
use deno_core::JsRuntime;
use deno_core::RuntimeOptions;
use log::{debug, error};
use std::fs;
use std::panic;
use std::path::PathBuf;
use std::rc::Rc;
use std::thread;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

pub mod env;
pub mod http_start;
pub mod module_loader;
pub mod net_override;
pub mod permissions;

use module_loader::DefaultModuleLoader;
use permissions::Permissions;

pub async fn serve(
    service_path: PathBuf,
    memory_limit_mb: u64,
    worker_timeout_ms: u64,
    tcp_stream: TcpStream,
) -> Result<(), Error> {
    let service_path_clone = service_path.clone();
    let service_name = service_path_clone.to_str().unwrap();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let (tcp_stream_tx, tcp_stream_rx) = mpsc::unbounded_channel::<TcpStream>();

    // start worker thread
    let _ = thread::spawn(move || {
        start_runtime(
            service_path,
            memory_limit_mb,
            worker_timeout_ms,
            tcp_stream_rx,
            shutdown_tx,
        )
    });

    tcp_stream_tx.send(tcp_stream)?;

    debug!("js worker for {} started", service_name);

    // wait for shutdown signal
    let _ = shutdown_rx.await;

    debug!("js worker for {} stopped", service_name);

    Ok(())
}

fn start_runtime(
    service_path: PathBuf,
    memory_limit_mb: u64,
    worker_timeout_ms: u64,
    tcp_stream_rx: mpsc::UnboundedReceiver<TcpStream>,
    shutdown_tx: oneshot::Sender<()>,
) {
    let user_agent = "supabase-edge-runtime".to_string();
    let extensions_with_js = vec![
        deno_webidl::init(),
        deno_console::init(),
        deno_url::init(),
        deno_web::init::<Permissions>(deno_web::BlobStore::default(), None),
        deno_fetch::init::<Permissions>(deno_fetch::Options {
            // TODO: update user agent
            user_agent: user_agent.clone(),
            ..Default::default()
        }),
        // TODO: support providing a custom seed for crypto
        deno_crypto::init(None),
        // TODO: configure root cert store
        deno_net::init::<Permissions>(None, false, None),
        deno_websocket::init::<Permissions>(user_agent.clone(), None, None),
        deno_http::init(),
        env::init(),
    ];
    let extensions = vec![
        net_override::init(),
        http_start::init(),
        permissions::init(),
    ];

    let mut js_runtime = JsRuntime::new(RuntimeOptions {
        extensions,
        extensions_with_js,
        module_loader: Some(Rc::new(DefaultModuleLoader)),
        is_main: true,
        create_params: Some(v8::CreateParams::default().heap_limits(
            mib_to_bytes(1) as usize,
            mib_to_bytes(memory_limit_mb) as usize,
        )),
        shared_array_buffer_store: None,
        compiled_wasm_module_store: None,
        ..Default::default()
    });

    let v8_thread_safe_handle = js_runtime.v8_isolate().thread_safe_handle();
    let (memory_limit_tx, memory_limit_rx) = mpsc::unbounded_channel::<u64>();

    // add a callback when a worker reaches its memory limit
    js_runtime.add_near_heap_limit_callback(move |cur, _init| {
        debug!(
            "low memory alert triggered {}",
            bytes_to_display(cur as u64)
        );
        let _ = memory_limit_tx.send(mib_to_bytes(memory_limit_mb));
        // add a 25% allowance to memory limit
        let cur = mib_to_bytes(memory_limit_mb + memory_limit_mb.div_euclid(4)) as usize;
        cur
    });

    // bootstrap the JS runtime
    let bootstrap_js = include_str!("./js_worker/js/bootstrap.js");
    js_runtime
        .execute_script("[js_worker]: bootstrap.js", bootstrap_js)
        .unwrap();

    debug!("bootstrapped function");

    //run inside a closure, so op_state_rc is released
    {
        let op_state_rc = js_runtime.op_state();
        let mut op_state = op_state_rc.borrow_mut();
        op_state.put::<mpsc::UnboundedReceiver<TcpStream>>(tcp_stream_rx);
    }

    let module_path = service_path.join("index.ts");
    // TODO: handle file missing error
    let main_module_code = fs::read_to_string(&module_path).unwrap();
    let main_module_url = Url::from_file_path(
        std::env::current_dir()
            .map(|p| p.join(&module_path))
            .unwrap(),
    )
    .unwrap();

    start_controller_thread(v8_thread_safe_handle, worker_timeout_ms, memory_limit_rx);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let future = async move {
        let mod_id = js_runtime
            .load_main_module(&main_module_url, Some(main_module_code))
            .await?;
        let result = js_runtime.mod_evaluate(mod_id);
        js_runtime.run_event_loop(false).await?;

        result.await?
    };
    let local = tokio::task::LocalSet::new();
    let res = local.block_on(&runtime, future);

    if res.is_err() {
        debug!("worker thread panicked {:?}", res.as_ref().err().unwrap());
    }
    shutdown_tx
        .send(())
        .expect("failed to send shutdown signal");
}

fn start_controller_thread(
    v8_thread_safe_handle: v8::IsolateHandle,
    worker_timeout_ms: u64,
    mut memory_limit_rx: mpsc::UnboundedReceiver<u64>,
) {
    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        // TODO: make it configurable
        let future = async move {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(worker_timeout_ms)) => {
                    error!("worker timed out. (duration {})", human_elapsed(worker_timeout_ms))
                }
                Some(val) = memory_limit_rx.recv() => {
                    error!("memory limit reached for worker. (used: {})", bytes_to_display(val))
                }
            }
        };
        rt.block_on(future);

        let ok = v8_thread_safe_handle.terminate_execution();
        if ok {
            debug!("worker terminated");
        }
    });
}
