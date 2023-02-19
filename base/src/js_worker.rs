use crate::utils::units::{bytes_to_display, human_elapsed, mib_to_bytes};

use anyhow::Error;
use deno_core::located_script_name;
use deno_core::url::Url;
use deno_core::JsRuntime;
use deno_core::RuntimeOptions;
use log::{debug, error};
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
pub mod runtime;

use module_loader::DefaultModuleLoader;
use permissions::Permissions;

pub async fn serve(
    service_path: PathBuf,
    memory_limit_mb: u64,
    worker_timeout_ms: u64,
    no_module_cache: bool,
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
            no_module_cache,
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
    no_module_cache: bool,
    tcp_stream_rx: mpsc::UnboundedReceiver<TcpStream>,
    shutdown_tx: oneshot::Sender<()>,
) {
    let user_agent = "supabase-edge-runtime".to_string();

    let module_path = service_path.join("index.ts");
    // TODO: handle file missing error
    let main_module_url = Url::from_file_path(
        std::env::current_dir()
            .map(|p| p.join(&module_path))
            .unwrap(),
    )
    .unwrap();

    // Note: this will load Mozilla's CAs (we may also need to support
    let root_cert_store = deno_tls::create_default_root_cert_store();

    let extensions_with_js = vec![
        deno_webidl::init(),
        deno_console::init(),
        deno_url::init(),
        deno_web::init::<Permissions>(deno_web::BlobStore::default(), None),
        deno_fetch::init::<Permissions>(deno_fetch::Options {
            user_agent: user_agent.clone(),
            ..Default::default()
        }),
        // TODO: support providing a custom seed for crypto
        deno_crypto::init(None),
        deno_net::init::<Permissions>(Some(root_cert_store.clone()), false, None),
        deno_websocket::init::<Permissions>(
            user_agent.clone(),
            Some(root_cert_store.clone()),
            None,
        ),
        deno_http::init(),
        deno_tls::init(),
        env::init(),
    ];
    let extensions = vec![
        net_override::init(),
        http_start::init(),
        permissions::init(),
        runtime::init(main_module_url.clone()),
    ];

    // FIXME: module_loader can panic
    let module_loader = DefaultModuleLoader::new(no_module_cache).unwrap();

    let mut js_runtime = JsRuntime::new(RuntimeOptions {
        extensions,
        extensions_with_js,
        module_loader: Some(Rc::new(module_loader)),
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

    // set bootstrap options
    let script = format!("globalThis.__build_target = \"{}\"", env!("TARGET"));
    js_runtime
        .execute_script(&located_script_name!(), &script)
        .expect("Failed to execute bootstrap script");

    // bootstrap the JS runtime
    let bootstrap_js = include_str!("./js_worker/js/bootstrap.js");
    js_runtime
        .execute_script("[js_worker]: bootstrap.js", bootstrap_js)
        .expect("Failed to execute bootstrap script");

    debug!("bootstrapped function");

    //run inside a closure, so op_state_rc is released
    {
        let op_state_rc = js_runtime.op_state();
        let mut op_state = op_state_rc.borrow_mut();
        op_state.put::<mpsc::UnboundedReceiver<TcpStream>>(tcp_stream_rx);
    }

    start_controller_thread(v8_thread_safe_handle, worker_timeout_ms, memory_limit_rx);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let future = async move {
        let mod_id = js_runtime.load_main_module(&main_module_url, None).await?;
        let result = js_runtime.mod_evaluate(mod_id);
        js_runtime.run_event_loop(false).await?;

        result.await?
    };
    let local = tokio::task::LocalSet::new();
    let res = local.block_on(&runtime, future);

    if res.is_err() {
        error!("worker thread panicked {:?}", res.as_ref().err().unwrap());
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

        let future = async move {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(worker_timeout_ms)) => {
                    debug!("max duration reached for the worker. terminating the worker. (duration {})", human_elapsed(worker_timeout_ms))
                }
                Some(val) = memory_limit_rx.recv() => {
                    error!("memory limit reached for the worker. terminating the worker. (used: {})", bytes_to_display(val))
                }
            }
        };
        rt.block_on(future);

        let ok = v8_thread_safe_handle.terminate_execution();
        if ok {
            debug!("worker terminated");
        } else {
            debug!("worker already terminated");
        }
    });
}
