use crate::utils::units::{bytes_to_display, human_elapsed, mib_to_bytes};

use anyhow::{anyhow, bail, Error};
use deno_core::located_script_name;
use deno_core::url::Url;
use deno_core::JsRuntime;
use deno_core::ModuleSpecifier;
use deno_core::RuntimeOptions;
use import_map::{parse_from_json, ImportMap, ImportMapDiagnostic};
use log::{debug, error, warn};
use std::collections::HashMap;
use std::fs;
use std::panic;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::thread;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

pub mod http_start;
pub mod module_loader;
pub mod net_override;
pub mod permissions;
pub mod runtime;
pub mod types;

use module_loader::DefaultModuleLoader;
use permissions::Permissions;
use supabase_edge_env::supabase_env;
use supabase_edge_worker_context::essentials::UserWorkerMsgs;
use supabase_edge_workers::supabase_user_workers;

fn load_import_map(maybe_path: Option<String>) -> Result<Option<ImportMap>, Error> {
    if let Some(path_str) = maybe_path {
        let path = Path::new(&path_str);
        let json_str = fs::read_to_string(path)?;

        let abs_path = std::env::current_dir().map(|p| p.join(&path))?;
        let base_url = Url::from_directory_path(abs_path.parent().unwrap()).unwrap();
        let result = parse_from_json(&base_url, json_str.as_str())?;
        print_import_map_diagnostics(&result.diagnostics);
        Ok(Some(result.import_map))
    } else {
        Ok(None)
    }
}

fn print_import_map_diagnostics(diagnostics: &[ImportMapDiagnostic]) {
    if !diagnostics.is_empty() {
        warn!(
            "Import map diagnostics:\n{}",
            diagnostics
                .iter()
                .map(|d| format!("  - {d}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

// FIXME: Extend a common worker for MainWorker and UserWorker

pub struct MainWorker {
    js_runtime: JsRuntime,
    main_module_url: ModuleSpecifier,
    worker_pool_tx: mpsc::UnboundedSender<supabase_edge_worker_context::essentials::UserWorkerMsgs>,
}

impl MainWorker {
    pub fn new(
        service_path: PathBuf,
        no_module_cache: bool,
        import_map_path: Option<String>,
        worker_pool_tx: mpsc::UnboundedSender<UserWorkerMsgs>,
    ) -> Result<Self, Error> {
        // Note: MainWorker
        // - does not have memory or worker timeout [x]
        // - has access to OS env vars [x]
        // - has access to local file system
        // - has access to WorkerPool resource (to create / launch workers)

        let user_agent = "supabase-edge-runtime".to_string();

        let base_url =
            Url::from_directory_path(std::env::current_dir().map(|p| p.join(&service_path))?)
                .unwrap();

        // TODO: check for other potential main paths (eg: index.js, index.tsx)
        let main_module_url = base_url.join("index.ts")?;

        // Note: this will load Mozilla's CAs (we may also need to support system certs)
        let root_cert_store = deno_tls::create_default_root_cert_store();

        let extensions = vec![
            deno_webidl::deno_webidl::init_ops_and_esm(),
            deno_console::deno_console::init_ops_and_esm(),
            deno_url::deno_url::init_ops_and_esm(),
            deno_web::deno_web::init_ops_and_esm::<Permissions>(
                deno_web::BlobStore::default(),
                None,
            ),
            deno_fetch::deno_fetch::init_ops_and_esm::<Permissions>(deno_fetch::Options {
                user_agent: user_agent.clone(),
                root_cert_store: Some(root_cert_store.clone()),
                ..Default::default()
            }),
            // TODO: support providing a custom seed for crypto
            deno_crypto::deno_crypto::init_ops_and_esm(None),
            deno_net::deno_net::init_ops_and_esm::<Permissions>(
                Some(root_cert_store.clone()),
                false,
                None,
            ),
            deno_websocket::deno_websocket::init_ops_and_esm::<Permissions>(
                user_agent.clone(),
                Some(root_cert_store.clone()),
                None,
            ),
            deno_http::deno_http::init_ops_and_esm(),
            deno_tls::deno_tls::init_ops_and_esm(),
            supabase_env::init_ops_and_esm(),
            supabase_user_workers::init_ops_and_esm(),
            net_override::init(),
            http_start::init(),
            permissions::init(),
            runtime::init(main_module_url.clone()),
        ];

        let import_map = load_import_map(import_map_path)?;
        let module_loader = DefaultModuleLoader::new(import_map, no_module_cache)?;

        let js_runtime = JsRuntime::new(RuntimeOptions {
            extensions,
            module_loader: Some(Rc::new(module_loader)),
            is_main: true,
            shared_array_buffer_store: None,
            compiled_wasm_module_store: None,
            ..Default::default()
        });

        Ok(Self {
            js_runtime,
            main_module_url,
            worker_pool_tx,
        })
    }

    pub fn snapshot() {
        unimplemented!();
    }

    pub fn run(
        mut self,
        stream: UnixStream,
        shutdown_tx: oneshot::Sender<()>,
    ) -> Result<(), Error> {
        // set bootstrap options
        let script = format!(r#"globalThis.__build_target = "{}"#, env!("TARGET"));
        self.js_runtime
            .execute_script::<String>(located_script_name!(), script.into())
            .expect("Failed to execute bootstrap script");

        // bootstrap the JS runtime
        let bootstrap_js = include_str!("./js_worker/js/main_bootstrap.js");
        self.js_runtime
            .execute_script("[main_worker]: main_bootstrap.js", &bootstrap_js[..])
            .expect("Failed to execute bootstrap script");

        debug!("bootstrapped function");

        let (unix_stream_tx, unix_stream_rx) = mpsc::unbounded_channel::<UnixStream>();
        if let Err(e) = unix_stream_tx.send(stream) {
            bail!(e)
        }

        //run inside a closure, so op_state_rc is released
        let env_vars = std::env::vars().collect();
        let worker_pool_tx = self.worker_pool_tx.clone();
        {
            let op_state_rc = self.js_runtime.op_state();
            let mut op_state = op_state_rc.borrow_mut();
            op_state.put::<mpsc::UnboundedReceiver<UnixStream>>(unix_stream_rx);
            op_state.put::<mpsc::UnboundedSender<UserWorkerMsgs>>(worker_pool_tx);
            op_state.put::<types::EnvVars>(env_vars);
        }

        let mut js_runtime = self.js_runtime;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let future = async move {
            let mod_id = js_runtime
                .load_main_module(&self.main_module_url, None)
                .await?;
            let mod_result = js_runtime.mod_evaluate(mod_id);

            let result = tokio::select! {
                _ = js_runtime.run_event_loop(false) => {
                    debug!("event loop completed");
                    mod_result.await?
                }
            };

            drop(js_runtime);
            result
        };

        let local = tokio::task::LocalSet::new();
        let res = local.block_on(&runtime, future);

        if res.is_err() {
            error!("worker thread panicked {:?}", res.as_ref().err().unwrap());
        }

        Ok(shutdown_tx.send(()).unwrap())
    }
}

pub struct UserWorker {
    js_runtime: JsRuntime,
    main_module_url: ModuleSpecifier,
    memory_limit_mb: u64,
    worker_timeout_ms: u64,
    env_vars: HashMap<String, String>,
}

impl UserWorker {
    pub fn new(
        service_path: PathBuf,
        memory_limit_mb: u64,
        worker_timeout_ms: u64,
        no_module_cache: bool,
        import_map_path: Option<String>,
        env_vars: HashMap<String, String>,
    ) -> Result<Self, Error> {
        let user_agent = "supabase-edge-runtime".to_string();

        let base_url =
            Url::from_directory_path(std::env::current_dir().map(|p| p.join(&service_path))?)
                .unwrap();

        // TODO: check for other potential main paths (eg: index.js, index.tsx)
        let main_module_url = base_url.join("index.ts")?;

        // Note: this will load Mozilla's CAs (we may also need to support system certs)
        let root_cert_store = deno_tls::create_default_root_cert_store();

        let extensions = vec![
            deno_webidl::deno_webidl::init_ops_and_esm(),
            deno_console::deno_console::init_ops_and_esm(),
            deno_url::deno_url::init_ops_and_esm(),
            deno_web::deno_web::init_ops_and_esm::<Permissions>(
                deno_web::BlobStore::default(),
                None,
            ),
            deno_fetch::deno_fetch::init_ops_and_esm::<Permissions>(deno_fetch::Options {
                user_agent: user_agent.clone(),
                root_cert_store: Some(root_cert_store.clone()),
                ..Default::default()
            }),
            // TODO: support providing a custom seed for crypto
            deno_crypto::deno_crypto::init_ops_and_esm(None),
            deno_net::deno_net::init_ops_and_esm::<Permissions>(
                Some(root_cert_store.clone()),
                false,
                None,
            ),
            deno_websocket::deno_websocket::init_ops_and_esm::<Permissions>(
                user_agent.clone(),
                Some(root_cert_store.clone()),
                None,
            ),
            deno_http::deno_http::init_ops_and_esm(),
            deno_tls::deno_tls::init_ops_and_esm(),
            supabase_env::init_ops_and_esm(),
            net_override::init(),
            http_start::init(),
            permissions::init(),
            runtime::init(main_module_url.clone()),
        ];

        let import_map = load_import_map(import_map_path)?;
        let module_loader = DefaultModuleLoader::new(import_map, no_module_cache)?;

        let js_runtime = JsRuntime::new(RuntimeOptions {
            extensions,
            module_loader: Some(Rc::new(module_loader)),
            is_main: true,
            create_params: Some(deno_core::v8::CreateParams::default().heap_limits(
                mib_to_bytes(1) as usize,
                mib_to_bytes(memory_limit_mb) as usize,
            )),
            shared_array_buffer_store: None,
            compiled_wasm_module_store: None,
            ..Default::default()
        });

        Ok(Self {
            js_runtime,
            main_module_url,
            memory_limit_mb,
            worker_timeout_ms,
            env_vars,
        })
    }

    pub fn snapshot() {
        unimplemented!();
    }

    pub fn run(
        mut self,
        stream: UnixStream,
        shutdown_tx: oneshot::Sender<()>,
    ) -> Result<(), Error> {
        let (memory_limit_tx, memory_limit_rx) = mpsc::unbounded_channel::<u64>();

        // add a callback when a worker reaches its memory limit
        let memory_limit_mb = self.memory_limit_mb;
        self.js_runtime
            .add_near_heap_limit_callback(move |cur, _init| {
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
        self.js_runtime
            .execute_script::<String>(&located_script_name!(), script.into())
            .expect("Failed to execute bootstrap script");

        // bootstrap the JS runtime
        //let bootstrap_js = include_str!("./js_worker/js/user_bootstrap.js");
        self.js_runtime
            .execute_script(
                "[user_worker]: user_bootstrap.js",
                include_str!("./js_worker/js/user_bootstrap.js"),
            )
            .expect("Failed to execute bootstrap script");

        debug!("bootstrapped function");

        let (unix_stream_tx, unix_stream_rx) = mpsc::unbounded_channel::<UnixStream>();
        if let Err(e) = unix_stream_tx.send(stream) {
            bail!(e)
        }

        //run inside a closure, so op_state_rc is released
        let env_vars = self.env_vars.clone();
        {
            let op_state_rc = self.js_runtime.op_state();
            let mut op_state = op_state_rc.borrow_mut();
            op_state.put::<mpsc::UnboundedReceiver<UnixStream>>(unix_stream_rx);
            op_state.put::<types::EnvVars>(env_vars);
        }

        let (halt_isolate_tx, mut halt_isolate_rx) = oneshot::channel::<()>();
        self.start_controller_thread(self.worker_timeout_ms, memory_limit_rx, halt_isolate_tx);

        let mut js_runtime = self.js_runtime;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let future = async move {
            let mod_id = js_runtime
                .load_main_module(&self.main_module_url, None)
                .await?;
            let mod_result = js_runtime.mod_evaluate(mod_id);

            let result = tokio::select! {
                _ = js_runtime.run_event_loop(false) => {
                    debug!("event loop completed");
                    mod_result.await?
                }
                _ = &mut halt_isolate_rx => {
                    debug!("worker exectution halted");
                    Ok(())
                }
            };

            drop(js_runtime);
            result
        };

        let local = tokio::task::LocalSet::new();
        let res = local.block_on(&runtime, future);

        if res.is_err() {
            error!("worker thread panicked {:?}", res.as_ref().err().unwrap());
        }

        Ok(shutdown_tx.send(()).unwrap())
    }

    fn start_controller_thread(
        &mut self,
        worker_timeout_ms: u64,
        mut memory_limit_rx: mpsc::UnboundedReceiver<u64>,
        halt_execution_tx: oneshot::Sender<()>,
    ) {
        let thread_safe_handle = self.js_runtime.v8_isolate().thread_safe_handle();

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
                        error!("memory limit reached for the worker. terminating the worker. (used: {})", bytes_to_display(val));
                        thread_safe_handle.terminate_execution();
                    }
                }
            };
            rt.block_on(future);

            if let Err(_) = halt_execution_tx.send(()) {
                error!("failed to send the halt execution signal");
            }
        });
    }
}
