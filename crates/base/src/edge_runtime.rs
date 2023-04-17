use crate::utils::units::{bytes_to_display, human_elapsed, mib_to_bytes};

use crate::js_worker::module_loader;
use anyhow::{anyhow, bail, Error};
use deno_core::error::AnyError;
use deno_core::url::Url;
use deno_core::JsRuntime;
use deno_core::ModuleSpecifier;
use deno_core::RuntimeOptions;
use deno_core::{located_script_name, serde_v8};
use import_map::{parse_from_json, ImportMap, ImportMapDiagnostic};
use log::{debug, error, warn};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::fs;
use std::panic;
use std::path::Path;
use std::rc::Rc;
use std::thread;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use crate::snapshot;
use module_loader::DefaultModuleLoader;
use sb_core::http_start::sb_core_http;
use sb_core::net::sb_core_net;
use sb_core::permissions::{sb_core_permissions, Permissions};
use sb_core::runtime::sb_core_runtime;
use sb_core::sb_core_main_js;
use sb_env::sb_env as sb_env_op;
use sb_worker_context::essentials::{
    EdgeContextInitOpts, EdgeContextOpts, EdgeUserRuntimeOpts, UserWorkerMsgs,
};
use sb_workers::sb_user_workers;

fn load_import_map(maybe_path: Option<String>) -> Result<Option<ImportMap>, Error> {
    if let Some(path_str) = maybe_path {
        let path = Path::new(&path_str);
        let json_str = fs::read_to_string(path)?;

        let abs_path = std::env::current_dir().map(|p| p.join(path))?;
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

pub struct EdgeRuntime {
    pub js_runtime: JsRuntime,
    pub main_module_url: ModuleSpecifier,
    pub is_user_runtime: bool,
    pub env_vars: HashMap<String, String>,
    pub conf: EdgeContextOpts,
    pub curr_user_opts: EdgeUserRuntimeOpts,
}

#[derive(Debug, PartialEq)]
pub enum EdgeCallResult {
    Unknown,
    TimeOut,
    HeapLimitReached,
    Completed,
}

impl EdgeRuntime {
    pub fn new(opts: EdgeContextInitOpts) -> Result<Self, Error> {
        let EdgeContextInitOpts {
            service_path,
            no_module_cache,
            import_map_path,
            env_vars,
            conf,
        } = opts;

        let (is_user_runtime, user_rt_opts) = match conf.clone() {
            EdgeContextOpts::UserWorker(conf) => (true, conf.clone()),
            EdgeContextOpts::MainWorker(_conf) => (false, EdgeUserRuntimeOpts::default()),
        };

        let user_agent = "supabase-edge-runtime".to_string();
        let base_url =
            Url::from_directory_path(std::env::current_dir().map(|p| p.join(&service_path))?)
                .unwrap();
        // TODO: check for other potential main paths (eg: index.js, index.tsx)
        let main_module_url = base_url.join("index.ts")?;

        // Note: this will load Mozilla's CAs (we may also need to support system certs)
        let root_cert_store = deno_tls::create_default_root_cert_store();

        let extensions = vec![
            sb_core_permissions::init_ops(),
            deno_webidl::deno_webidl::init_ops(),
            deno_console::deno_console::init_ops(),
            deno_url::deno_url::init_ops(),
            deno_web::deno_web::init_ops::<Permissions>(deno_web::BlobStore::default(), None),
            deno_fetch::deno_fetch::init_ops::<Permissions>(deno_fetch::Options {
                user_agent: user_agent.clone(),
                root_cert_store: Some(root_cert_store.clone()),
                ..Default::default()
            }),
            deno_websocket::deno_websocket::init_ops::<Permissions>(
                user_agent,
                Some(root_cert_store.clone()),
                None,
            ),
            // TODO: support providing a custom seed for crypto
            deno_crypto::deno_crypto::init_ops(None),
            deno_net::deno_net::init_ops::<Permissions>(Some(root_cert_store), false, None),
            deno_tls::deno_tls::init_ops(),
            deno_http::deno_http::init_ops(),
            sb_env_op::init_ops(),
            sb_user_workers::init_ops(),
            sb_core_main_js::init_ops(),
            sb_core_net::init_ops(),
            sb_core_http::init_ops(),
            sb_core_runtime::init_ops(Some(main_module_url.clone())),
        ];

        let import_map = load_import_map(import_map_path)?;
        let module_loader = DefaultModuleLoader::new(import_map, no_module_cache)?;

        let mut js_runtime = JsRuntime::new(RuntimeOptions {
            extensions,
            module_loader: Some(Rc::new(module_loader)),
            is_main: true,
            create_params: {
                if is_user_runtime {
                    Some(deno_core::v8::CreateParams::default().heap_limits(
                        mib_to_bytes(1) as usize,
                        mib_to_bytes(user_rt_opts.memory_limit_mb) as usize,
                    ))
                } else {
                    None
                }
            },
            shared_array_buffer_store: None,
            compiled_wasm_module_store: None,
            startup_snapshot: Some(snapshot::snapshot()),
            ..Default::default()
        });

        // Bootstrapping stage
        let script = format!(
            "globalThis.bootstrapSBEdge({}, {})",
            deno_core::serde_json::json!({ "target": env!("TARGET") }),
            is_user_runtime
        );

        js_runtime
            .execute_script::<String>(&located_script_name!(), script.into())
            .expect("Failed to execute bootstrap script");

        {
            //run inside a closure, so op_state_rc is released
            let env_vars = env_vars.clone();
            let op_state_rc = js_runtime.op_state();
            let mut op_state = op_state_rc.borrow_mut();
            op_state.put::<sb_env::EnvVars>(env_vars);
        }

        Ok(Self {
            js_runtime,
            main_module_url,
            is_user_runtime,
            env_vars,
            conf,
            curr_user_opts: user_rt_opts,
        })
    }

    pub async fn run(
        mut self,
        stream: UnixStream,
        shutdown_tx: oneshot::Sender<()>,
    ) -> Result<EdgeCallResult, Error> {
        let is_user_rt = self.is_user_runtime;

        let (unix_stream_tx, unix_stream_rx) = mpsc::unbounded_channel::<UnixStream>();
        if let Err(e) = unix_stream_tx.send(stream) {
            bail!(e)
        }

        {
            let op_state_rc = self.js_runtime.op_state();
            let mut op_state = op_state_rc.borrow_mut();
            op_state.put::<mpsc::UnboundedReceiver<UnixStream>>(unix_stream_rx);

            if !is_user_rt {
                if let EdgeContextOpts::MainWorker(conf) = self.conf.clone() {
                    op_state
                        .put::<mpsc::UnboundedSender<UserWorkerMsgs>>(conf.worker_pool_tx.clone());
                }
            }
        }

        let (halt_isolate_tx, mut halt_isolate_rx) = oneshot::channel::<EdgeCallResult>();

        if is_user_rt {
            let (memory_limit_tx, memory_limit_rx) = mpsc::unbounded_channel::<u64>();

            // add a callback when a worker reaches its memory limit
            let memory_limit_mb = self.curr_user_opts.memory_limit_mb;
            self.js_runtime
                .add_near_heap_limit_callback(move |cur, _init| {
                    debug!(
                        "[{}] Low memory alert triggered: {}",
                        "x",
                        bytes_to_display(cur as u64),
                    );
                    let _ = memory_limit_tx.send(mib_to_bytes(memory_limit_mb));
                    // add a 25% allowance to memory limit
                    let cur =
                        mib_to_bytes(memory_limit_mb + memory_limit_mb.div_euclid(4)) as usize;
                    cur
                });

            self.start_controller_thread(
                self.curr_user_opts.worker_timeout_ms,
                memory_limit_rx,
                halt_isolate_tx,
            );
        }

        let mut js_runtime = self.js_runtime;

        let future = async move {
            let mod_id = js_runtime
                .load_main_module(&self.main_module_url, None)
                .await?;
            let mod_result = js_runtime.mod_evaluate(mod_id);

            let result: Result<EdgeCallResult, Error> = tokio::select! {
                _ = js_runtime.run_event_loop(false) => {
                    debug!("Event loop has completed");
                    let _ = mod_result.await?;

                    Ok(EdgeCallResult::Completed)
                },
                call_result = &mut halt_isolate_rx => {
                    debug!("User Worker execution halted");

                    Ok(call_result.unwrap_or(EdgeCallResult::Unknown))
                }
            };

            drop(js_runtime);
            result
        };

        let res = future.await;

        if res.is_err() {
            println!("worker thread panicked {:?}", res.as_ref().err().unwrap());
        }

        shutdown_tx.send(()).unwrap();
        res
    }

    fn start_controller_thread(
        &mut self,
        worker_timeout_ms: u64,
        mut memory_limit_rx: mpsc::UnboundedReceiver<u64>,
        halt_isolate_tx: oneshot::Sender<EdgeCallResult>,
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
                        debug!("max duration reached for the worker. terminating the worker. (duration {})", human_elapsed(worker_timeout_ms));
                        thread_safe_handle.terminate_execution();
                        return EdgeCallResult::TimeOut;
                    }
                    Some(val) = memory_limit_rx.recv() => {
                        error!("memory limit reached for the worker. terminating the worker. (used: {})", bytes_to_display(val));
                        thread_safe_handle.terminate_execution();
                        return EdgeCallResult::HeapLimitReached;
                    }
                }
            };
            let call = rt.block_on(future);

            if let Err(_) = halt_isolate_tx.send(call) {
                error!("failed to send the halt execution signal");
            }
        });
    }

    fn to_value<T>(
        &mut self,
        global_value: &deno_core::v8::Global<deno_core::v8::Value>,
    ) -> Result<T, AnyError>
    where
        T: DeserializeOwned + 'static,
    {
        let scope = &mut self.js_runtime.handle_scope();
        let value = deno_core::v8::Local::new(scope, global_value.clone());
        Ok(serde_v8::from_v8(scope, value)?)
    }
}

#[cfg(test)]
mod test {
    use crate::edge_runtime::{EdgeCallResult, EdgeRuntime};
    use deno_core::v8::{ContextScope, HandleScope, Local, Object};
    use deno_core::{serde_v8, JsRuntime};
    use sb_worker_context::essentials::{
        EdgeContextInitOpts, EdgeContextOpts, EdgeMainRuntimeOpts, EdgeUserRuntimeOpts,
        UserWorkerMsgs,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tokio::net::UnixStream;
    use tokio::sync::mpsc::UnboundedSender;
    use tokio::sync::oneshot::Sender;
    use tokio::sync::{mpsc, oneshot};

    fn create_runtime(
        path: Option<PathBuf>,
        env_vars: Option<HashMap<String, String>>,
        user_conf: Option<EdgeContextOpts>,
    ) -> EdgeRuntime {
        let (worker_pool_tx, _) = mpsc::unbounded_channel::<UserWorkerMsgs>();
        let mut runtime = EdgeRuntime::new(EdgeContextInitOpts {
            service_path: path.unwrap_or(PathBuf::from("./examples/main")),
            no_module_cache: false,
            import_map_path: None,
            env_vars: env_vars.unwrap_or(Default::default()),
            conf: {
                if user_conf.is_some() {
                    user_conf.unwrap()
                } else {
                    EdgeContextOpts::MainWorker(EdgeMainRuntimeOpts {
                        worker_pool_tx: worker_pool_tx,
                    })
                }
            },
        })
        .unwrap();

        runtime
    }

    fn create_user_rt_params_to_run() -> (UnixStream, Sender<()>) {
        let (sender_stream, recv_stream) = UnixStream::pair().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        (recv_stream, shutdown_tx)
    }

    // Main Runtime should have access to `EdgeRuntime`
    #[tokio::test]
    async fn test_main_runtime_creation() {
        let mut runtime = create_runtime(None, None, None);

        {
            let scope = &mut runtime.js_runtime.handle_scope();
            let context = scope.get_current_context();
            let inner_scope = &mut deno_core::v8::ContextScope::new(scope, context);
            let global = context.global(inner_scope);
            let edge_runtime_key: deno_core::v8::Local<deno_core::v8::Value> =
                deno_core::serde_v8::to_v8(inner_scope, "EdgeRuntime").unwrap();
            assert_eq!(
                global
                    .get(inner_scope, edge_runtime_key)
                    .unwrap()
                    .is_undefined(),
                false
            );
        }
    }

    // User Runtime Should not have access to EdgeRuntime
    #[tokio::test]
    async fn test_user_runtime_creation() {
        let mut runtime = create_runtime(
            None,
            None,
            Some(EdgeContextOpts::UserWorker(Default::default())),
        );

        {
            let scope = &mut runtime.js_runtime.handle_scope();
            let context = scope.get_current_context();
            let inner_scope = &mut deno_core::v8::ContextScope::new(scope, context);
            let global = context.global(inner_scope);
            let edge_runtime_key: deno_core::v8::Local<deno_core::v8::Value> =
                deno_core::serde_v8::to_v8(inner_scope, "EdgeRuntime").unwrap();
            assert_eq!(
                global
                    .get(inner_scope, edge_runtime_key)
                    .unwrap()
                    .is_undefined(),
                true
            );
        }
    }

    #[tokio::test]
    async fn test_os_env_vars() {
        std::env::set_var("Supa_Test", "Supa_Value");
        let mut main_rt = create_runtime(None, Some(std::env::vars().collect()), None);
        let mut user_rt = create_runtime(
            None,
            None,
            Some(EdgeContextOpts::UserWorker(Default::default())),
        );
        assert!(main_rt.env_vars.len() > 0);
        assert_eq!(user_rt.env_vars.len(), 0);

        let err = main_rt
            .js_runtime
            .execute_script(
                "<anon>",
                r#"
            // Should not be able to set
            Deno.env.set("Supa_Test", "Supa_Value");
        "#,
            )
            .err()
            .unwrap();
        assert!(err
            .to_string()
            .contains("Error: The operation is not supported"));

        let main_deno_env_get_supa_test = main_rt
            .js_runtime
            .execute_script(
                "<anon>",
                r#"
            // Should not be able to set
            Deno.env.get("Supa_Test");
        "#,
            )
            .unwrap();
        let serde_deno_env =
            main_rt.to_value::<deno_core::serde_json::Value>(&main_deno_env_get_supa_test);
        assert_eq!(serde_deno_env.unwrap().as_str().unwrap(), "Supa_Value");

        // User does not have this env variable because it was not provided
        // During the runtime creation
        let user_deno_env_get_supa_test = user_rt
            .js_runtime
            .execute_script(
                "<anon>",
                r#"
            // Should not be able to set
            Deno.env.get("Supa_Test");
        "#,
            )
            .unwrap();
        let user_serde_deno_env =
            user_rt.to_value::<deno_core::serde_json::Value>(&user_deno_env_get_supa_test);
        assert_eq!(user_serde_deno_env.unwrap().is_null(), true);
    }

    #[tokio::test]
    async fn test_timeout_infinite_loop() {
        let mut user_rt = create_runtime(
            Some(PathBuf::from("/Users/andrespirela/Documents/workspace/supabase/edge-runtime/examples/infinite_loop")),
            None,
            Some(EdgeContextOpts::UserWorker(EdgeUserRuntimeOpts {
                memory_limit_mb: 100,
                worker_timeout_ms: 1000,
                id: "".to_string()
            })),
        );
        let (stream, shutdown) = create_user_rt_params_to_run();
        let data = user_rt.run(stream, shutdown).await.unwrap();
        assert_eq!(data, EdgeCallResult::TimeOut);
    }
}
