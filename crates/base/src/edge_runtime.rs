use crate::utils::units::{bytes_to_display, human_elapsed, mib_to_bytes};

use crate::js_worker::module_loader;
use anyhow::{anyhow, Error};
use deno_core::error::AnyError;
use deno_core::url::Url;
use deno_core::{located_script_name, serde_v8, JsRuntime, ModuleId, RuntimeOptions};
use import_map::{parse_from_json, ImportMap, ImportMapDiagnostic};
use log::{debug, error, warn};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::panic;
use std::path::Path;
use std::rc::Rc;
use std::thread;
use std::time::Duration;
use std::{fmt, fs};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use urlencoding::decode;

use crate::utils::send_event_if_event_manager_available;
use crate::{errors_rt, snapshot};
use module_loader::DefaultModuleLoader;
use sb_core::http_start::sb_core_http;
use sb_core::net::sb_core_net;
use sb_core::permissions::{sb_core_permissions, Permissions, RuntimeNodeEnv};
use sb_core::runtime::sb_core_runtime;
use sb_core::sb_core_main_js;
use sb_env::sb_env as sb_env_op;
use sb_worker_context::essentials::{
    EdgeContextInitOpts, EdgeContextOpts, EdgeEventRuntimeOpts, EdgeUserRuntimeOpts, UserWorkerMsgs,
};
use sb_worker_context::events::{PseudoEvent, WorkerEvents};
use sb_workers::events::sb_user_event_worker;
use sb_workers::sb_user_workers;

fn load_import_map(maybe_path: Option<String>) -> Result<Option<ImportMap>, Error> {
    if let Some(path_str) = maybe_path {
        let json_str;
        let base_url;

        // check if the path is a data URI (prefixed with data:)
        // the data URI takes the following format
        // data:{encodeURIComponent(mport_map.json)?{encodeURIComponent(base_path)}
        if path_str.starts_with("data:") {
            let data_uri = Url::parse(&path_str)?;
            json_str = decode(data_uri.path())?.into_owned();
            base_url =
                Url::from_directory_path(decode(data_uri.query().unwrap_or(""))?.into_owned())
                    .map_err(|_| anyhow!("invalid import map base url"))?;
        } else {
            let path = Path::new(&path_str);
            let abs_path = std::env::current_dir().map(|p| p.join(path))?;
            json_str = fs::read_to_string(abs_path.clone())?;
            base_url = Url::from_directory_path(abs_path.parent().unwrap())
                .map_err(|_| anyhow!("invalid import map base url"))?;
        }

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
    pub main_module_id: ModuleId,
    pub is_user_runtime: bool,
    pub is_event_worker: bool,
    pub env_vars: HashMap<String, String>,
    pub conf: EdgeContextOpts,
    pub curr_user_opts: EdgeUserRuntimeOpts,
}

pub struct EdgeRuntimeError(Error);

impl PartialEq for EdgeRuntimeError {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_string() == other.0.to_string()
    }
}

impl fmt::Debug for EdgeRuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[Js Error] {}", self.0)
    }
}

#[derive(Debug, PartialEq)]
pub enum EdgeCallResult {
    Unknown,
    TimeOut,
    ModuleEvaluationTimedOut,
    HeapLimitReached,
    Completed,
    ErrorThrown(EdgeRuntimeError),
}

fn get_error_class_name(e: &AnyError) -> &'static str {
    errors_rt::get_error_class_name(e).unwrap_or("Error")
}

impl EdgeRuntime {
    pub async fn new(
        opts: EdgeContextInitOpts,
        event_manager_opts: Option<EdgeEventRuntimeOpts>,
    ) -> Result<Self, Error> {
        let mut maybe_events_msg_tx: Option<mpsc::UnboundedSender<WorkerEvents>> = None;

        let EdgeContextInitOpts {
            service_path,
            no_module_cache,
            import_map_path,
            env_vars,
            conf,
        } = opts;

        let (is_user_runtime, is_event_worker, user_rt_opts) = match conf.clone() {
            EdgeContextOpts::UserWorker(conf) => (true, false, conf),
            EdgeContextOpts::MainWorker(_conf) => (false, false, EdgeUserRuntimeOpts::default()),
            EdgeContextOpts::EventsWorker => (false, true, EdgeUserRuntimeOpts::default()), // TODO: This needs to be an option (user opts)
        };

        if is_user_runtime {
            if let EdgeContextOpts::UserWorker(conf) = conf.clone() {
                if let Some(events_msg_tx) = conf.events_msg_tx {
                    maybe_events_msg_tx = Some(events_msg_tx)
                }
            }
        }

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
            deno_io::deno_io::init_ops(Some(Default::default())),
            deno_fs::deno_fs::init_ops::<Permissions>(false),
            deno_flash::deno_flash::init_ops::<Permissions>(false),
            sb_env_op::init_ops(),
            sb_os::sb_os::init_ops(),
            sb_user_workers::init_ops(),
            sb_user_event_worker::init_ops(),
            sb_core_main_js::init_ops(),
            sb_core_net::init_ops(),
            sb_core_http::init_ops(),
            sb_node::deno_node::init_ops::<RuntimeNodeEnv>(None),
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
                        mib_to_bytes(0) as usize,
                        mib_to_bytes(user_rt_opts.memory_limit_mb) as usize,
                    ))
                } else {
                    None
                }
            },
            get_error_class_fn: Some(&get_error_class_name),
            shared_array_buffer_store: None,
            compiled_wasm_module_store: None,
            startup_snapshot: Some(snapshot::snapshot()),
            ..Default::default()
        });

        // Bootstrapping stage
        let script = format!(
            "globalThis.bootstrapSBEdge({}, {}, {})",
            deno_core::serde_json::json!({ "target": env!("TARGET") }),
            is_user_runtime,
            is_event_worker
        );

        js_runtime
            .execute_script::<String>(located_script_name!(), script)
            .expect("Failed to execute bootstrap script");

        {
            //run inside a closure, so op_state_rc is released
            let env_vars = env_vars.clone();
            let op_state_rc = js_runtime.op_state();
            let mut op_state = op_state_rc.borrow_mut();
            op_state.put::<sb_env::EnvVars>(env_vars);

            if is_event_worker {
                if let EdgeContextOpts::EventsWorker = conf.clone() {
                    // We unwrap because event_manager_opts must always be present when type is `EventsWorker`
                    op_state.put::<mpsc::UnboundedReceiver<WorkerEvents>>(
                        event_manager_opts.unwrap().event_rx,
                    );
                }
            }

            if is_user_runtime {
                if let Some(events_msg_tx) = maybe_events_msg_tx.clone() {
                    op_state.put::<mpsc::UnboundedSender<WorkerEvents>>(events_msg_tx);
                }
            }
        }

        let main_module_id = js_runtime.load_main_module(&main_module_url, None).await?;

        Ok(Self {
            js_runtime,
            main_module_id,
            is_user_runtime,
            env_vars,
            conf,
            curr_user_opts: user_rt_opts,
            is_event_worker,
        })
    }

    pub fn event_sender(&self) -> Option<mpsc::UnboundedSender<WorkerEvents>> {
        if self.is_user_runtime {
            if let EdgeContextOpts::UserWorker(conf) = self.conf.clone() {
                conf.events_msg_tx
            } else {
                None
            }
        } else {
            None
        }
    }

    pub async fn run(
        mut self,
        unix_stream_rx: mpsc::UnboundedReceiver<UnixStream>,
    ) -> Result<EdgeCallResult, Error> {
        let is_user_rt = self.is_user_runtime;

        {
            let op_state_rc = self.js_runtime.op_state();
            let mut op_state = op_state_rc.borrow_mut();
            op_state.put::<mpsc::UnboundedReceiver<UnixStream>>(unix_stream_rx);

            if !is_user_rt {
                if let EdgeContextOpts::MainWorker(conf) = self.conf.clone() {
                    op_state.put::<mpsc::UnboundedSender<UserWorkerMsgs>>(conf.worker_pool_tx);
                }
            }
        }

        let (halt_isolate_tx, mut halt_isolate_rx) = oneshot::channel::<EdgeCallResult>();

        if is_user_rt {
            let (memory_limit_tx, memory_limit_rx) = mpsc::unbounded_channel::<u64>();

            self.start_controller_thread(
                self.curr_user_opts.worker_timeout_ms,
                memory_limit_rx,
                halt_isolate_tx,
            );

            // add a callback when a worker reaches its memory limit
            let memory_limit_mb = self.curr_user_opts.memory_limit_mb;
            self.js_runtime.add_near_heap_limit_callback(move |cur, _| {
                debug!(
                    "Low memory alert triggered: {}",
                    bytes_to_display(cur as u64),
                );

                let _ = memory_limit_tx.send(mib_to_bytes(memory_limit_mb));

                (cur + memory_limit_mb as usize) << 20
            });
        }

        let mut js_runtime = self.js_runtime;

        let future = async move {
            let mod_result = js_runtime.mod_evaluate(self.main_module_id);

            let result: Result<EdgeCallResult, Error> = tokio::select! {
                _ = js_runtime.run_event_loop(false) => {
                    debug!("Event loop has completed");

                    match tokio::time::timeout(std::time::Duration::from_millis(self.curr_user_opts.worker_timeout_ms), mod_result).await {
                        Err(_err) => {
                            return Ok(EdgeCallResult::ModuleEvaluationTimedOut);
                        },
                        Ok(Ok(evaluation_result)) => {
                            if let Err(e) = evaluation_result {
                                let err_as_str = e.to_string();
                                debug!("{}", err_as_str);
                                return Ok(EdgeCallResult::ErrorThrown(EdgeRuntimeError(e)));
                            }
                        },
                        Ok(Err(_)) => { return Ok(EdgeCallResult::Unknown); }
                    }

                    Ok(EdgeCallResult::Completed)
                },
                // TODO: Fix race condition
                call_result = &mut halt_isolate_rx => {
                    debug!("User Worker execution halted");
                    Ok(call_result.unwrap_or(EdgeCallResult::Unknown))
                }
            };

            drop(js_runtime);
            result
        };

        future.await
    }

    fn start_controller_thread(
        &mut self,
        worker_timeout_ms: u64,
        mut memory_limit_rx: mpsc::UnboundedReceiver<u64>,
        halt_isolate_tx: oneshot::Sender<EdgeCallResult>,
    ) {
        let thread_safe_handle = self.js_runtime.v8_isolate().thread_safe_handle();
        let event_sender = self.event_sender();

        thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            let future = async move {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(worker_timeout_ms)) => {
                        debug!("max duration reached for the worker. terminating the worker. (duration {})", human_elapsed(worker_timeout_ms));
                        send_event_if_event_manager_available(event_sender, WorkerEvents::TimeLimit(PseudoEvent {}));
                        thread_safe_handle.terminate_execution();
                        EdgeCallResult::TimeOut
                    }
                    Some(val) = memory_limit_rx.recv() => {
                        error!("memory limit reached for the worker. terminating the worker. (used: {})", bytes_to_display(val));
                        send_event_if_event_manager_available(event_sender, WorkerEvents::MemoryLimit(PseudoEvent {}));
                        thread_safe_handle.terminate_execution();
                        EdgeCallResult::HeapLimitReached
                    }
                }
            };
            let call = rt.block_on(future);

            // send a message to halt the isolate
            if halt_isolate_tx.send(call).is_err() {
                error!("failed to send the halt execution signal");
            }
        });
    }

    #[allow(clippy::wrong_self_convention)]
    // TODO: figure out why rustc complains about this
    #[allow(dead_code)]
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
    use flaky_test::flaky_test;
    use sb_worker_context::essentials::{
        EdgeContextInitOpts, EdgeContextOpts, EdgeMainRuntimeOpts, EdgeUserRuntimeOpts,
        UserWorkerMsgs,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tokio::net::UnixStream;
    use tokio::sync::mpsc;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

    async fn create_runtime(
        path: Option<PathBuf>,
        env_vars: Option<HashMap<String, String>>,
        user_conf: Option<EdgeContextOpts>,
    ) -> EdgeRuntime {
        let (worker_pool_tx, _) = mpsc::unbounded_channel::<UserWorkerMsgs>();

        EdgeRuntime::new(
            EdgeContextInitOpts {
                service_path: path.unwrap_or(PathBuf::from("./test_cases/main")),
                no_module_cache: false,
                import_map_path: None,
                env_vars: env_vars.unwrap_or(Default::default()),
                conf: {
                    if let Some(uc) = user_conf {
                        uc
                    } else {
                        EdgeContextOpts::MainWorker(EdgeMainRuntimeOpts { worker_pool_tx })
                    }
                },
            },
            None,
        )
        .await
        .unwrap()
    }

    fn create_user_rt_params_to_run() -> (UnboundedSender<UnixStream>, UnboundedReceiver<UnixStream>)
    {
        mpsc::unbounded_channel::<UnixStream>()
    }

    // Main Runtime should have access to `EdgeRuntime`
    #[tokio::test]
    async fn test_main_runtime_creation() {
        let mut runtime = create_runtime(None, None, None).await;

        {
            let scope = &mut runtime.js_runtime.handle_scope();
            let context = scope.get_current_context();
            let inner_scope = &mut deno_core::v8::ContextScope::new(scope, context);
            let global = context.global(inner_scope);
            let edge_runtime_key: deno_core::v8::Local<deno_core::v8::Value> =
                deno_core::serde_v8::to_v8(inner_scope, "EdgeRuntime").unwrap();
            assert!(!global
                .get(inner_scope, edge_runtime_key)
                .unwrap()
                .is_undefined(),);
        }
    }

    // User Runtime Should not have access to EdgeRuntime
    #[tokio::test]
    async fn test_user_runtime_creation() {
        let mut runtime = create_runtime(
            None,
            None,
            Some(EdgeContextOpts::UserWorker(Default::default())),
        )
        .await;

        {
            let scope = &mut runtime.js_runtime.handle_scope();
            let context = scope.get_current_context();
            let inner_scope = &mut deno_core::v8::ContextScope::new(scope, context);
            let global = context.global(inner_scope);
            let edge_runtime_key: deno_core::v8::Local<deno_core::v8::Value> =
                deno_core::serde_v8::to_v8(inner_scope, "EdgeRuntime").unwrap();
            assert!(global
                .get(inner_scope, edge_runtime_key)
                .unwrap()
                .is_undefined(),);
        }
    }

    #[tokio::test]
    async fn test_main_rt_fs() {
        let mut main_rt = create_runtime(None, Some(std::env::vars().collect()), None).await;

        let global_value_deno_read_file_script = main_rt
            .js_runtime
            .execute_script(
                "<anon>",
                r#"
            Deno.readTextFileSync("./test_cases/readFile/hello_world.json");
        "#,
            )
            .unwrap();
        let fs_read_result =
            main_rt.to_value::<deno_core::serde_json::Value>(&global_value_deno_read_file_script);
        assert_eq!(
            fs_read_result.unwrap().as_str().unwrap(),
            "{\n  \"hello\": \"world\"\n}"
        );
    }

    #[tokio::test]
    async fn test_node_builtin_imports() {
        let mut main_rt = create_runtime(
            Some(PathBuf::from("./test_cases/node-built-in")),
            Some(std::env::vars().collect()),
            None,
        )
        .await;
        let mod_evaluate = main_rt.js_runtime.mod_evaluate(main_rt.main_module_id);
        let _ = main_rt.js_runtime.run_event_loop(false).await;
        let global_value_deno_read_file_script = main_rt
            .js_runtime
            .execute_script(
                "<anon>",
                r#"
            globalThis.basename('/Users/Refsnes/demo_path.js');
        "#,
            )
            .unwrap();
        let fs_read_result =
            main_rt.to_value::<deno_core::serde_json::Value>(&global_value_deno_read_file_script);
        assert_eq!(fs_read_result.unwrap().as_str().unwrap(), "demo_path.js");
        std::mem::drop(mod_evaluate);
    }

    #[tokio::test]
    async fn test_os_ops() {
        let mut user_rt = create_runtime(
            None,
            None,
            Some(EdgeContextOpts::UserWorker(Default::default())),
        )
        .await;

        let user_rt_execute_scripts = user_rt
            .js_runtime
            .execute_script(
                "<anon>",
                r#"
            // Should not be able to set
            const data = {
                gid: Deno.gid(),
                uid: Deno.uid(),
                hostname: Deno.hostname(),
                loadavg: Deno.loadavg(),
                osUptime: Deno.osUptime(),
                osRelease: Deno.osRelease(),
                systemMemoryInfo: Deno.systemMemoryInfo(),
                consoleSize: Deno.consoleSize(),
                version: [Deno.version.deno, Deno.version.v8, Deno.version.typescript],
                networkInterfaces: Deno.networkInterfaces()
            };
            data;
        "#,
            )
            .unwrap();
        let serde_deno_env = user_rt
            .to_value::<deno_core::serde_json::Value>(&user_rt_execute_scripts)
            .unwrap();
        assert_eq!(serde_deno_env.get("gid").unwrap().as_i64().unwrap(), 1000);
        assert_eq!(serde_deno_env.get("uid").unwrap().as_i64().unwrap(), 1000);
        assert!(serde_deno_env.get("osUptime").unwrap().as_i64().unwrap() > 0);
        assert_eq!(
            serde_deno_env.get("osRelease").unwrap().as_str().unwrap(),
            "0.0.0-00000000-generic"
        );

        let loadavg_array = serde_deno_env
            .get("loadavg")
            .unwrap()
            .as_array()
            .unwrap()
            .to_vec();
        assert_eq!(loadavg_array.get(0).unwrap().as_f64().unwrap(), 0.0);
        assert_eq!(loadavg_array.get(1).unwrap().as_f64().unwrap(), 0.0);
        assert_eq!(loadavg_array.get(2).unwrap().as_f64().unwrap(), 0.0);

        let network_interfaces_data = serde_deno_env
            .get("networkInterfaces")
            .unwrap()
            .as_array()
            .unwrap()
            .to_vec();
        assert_eq!(network_interfaces_data.len(), 2);

        let deno_version_array = serde_deno_env
            .get("version")
            .unwrap()
            .as_array()
            .unwrap()
            .to_vec();
        assert_eq!(deno_version_array.get(0).unwrap().as_str().unwrap(), "");
        assert_eq!(deno_version_array.get(1).unwrap().as_str().unwrap(), "");
        assert_eq!(deno_version_array.get(2).unwrap().as_str().unwrap(), "");

        let system_memory_info_map = serde_deno_env
            .get("systemMemoryInfo")
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        assert!(system_memory_info_map.contains_key("total"));
        assert!(system_memory_info_map.contains_key("free"));
        assert!(system_memory_info_map.contains_key("available"));
        assert!(system_memory_info_map.contains_key("buffers"));
        assert!(system_memory_info_map.contains_key("cached"));
        assert!(system_memory_info_map.contains_key("swapTotal"));
        assert!(system_memory_info_map.contains_key("swapFree"));

        let deno_consle_size_map = serde_deno_env
            .get("consoleSize")
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        assert!(deno_consle_size_map.contains_key("rows"));
        assert!(deno_consle_size_map.contains_key("columns"));

        let user_rt_execute_scripts = user_rt.js_runtime.execute_script(
            "<anon>",
            r#"
            let cmd = new Deno.Command("", {});
            cmd.outputSync();
        "#,
        );
        assert!(user_rt_execute_scripts.is_err());
        assert!(user_rt_execute_scripts
            .unwrap_err()
            .to_string()
            .contains("Spawning subprocesses is not allowed on Supabase Edge Runtime"));
    }

    #[tokio::test]
    async fn test_os_env_vars() {
        std::env::set_var("Supa_Test", "Supa_Value");
        let mut main_rt = create_runtime(None, Some(std::env::vars().collect()), None).await;
        let mut user_rt = create_runtime(
            None,
            None,
            Some(EdgeContextOpts::UserWorker(Default::default())),
        )
        .await;
        assert!(!main_rt.env_vars.is_empty());
        assert!(user_rt.env_vars.is_empty());

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
            .contains("NotSupported: The operation is not supported"));

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
        assert!(user_serde_deno_env.unwrap().is_null());
    }

    async fn create_basic_user_runtime(
        path: &str,
        memory_limit: u64,
        worker_timeout_ms: u64,
    ) -> EdgeRuntime {
        create_runtime(
            Some(PathBuf::from(path)),
            None,
            Some(EdgeContextOpts::UserWorker(EdgeUserRuntimeOpts {
                memory_limit_mb: memory_limit,
                worker_timeout_ms,
                force_create: true,
                key: None,
                pool_msg_tx: None,
                events_msg_tx: None,
            })),
        )
        .await
    }

    // FIXME: Disabling these tests since they are flaky in CI
    //#[tokio::test]
    //async fn test_timeout_infinite_promises() {
    //    let user_rt = create_basic_user_runtime("./test_cases/infinite_promises", 100, 1000);
    //    let (_tx, unix_stream_rx) = create_user_rt_params_to_run();
    //    let data = user_rt.run(unix_stream_rx).await.unwrap();
    //    assert_eq!(data, EdgeCallResult::ModuleEvaluationTimedOut);
    //}

    //#[flaky_test]
    //async fn test_timeout_infinite_loop() {
    //    let user_rt = create_basic_user_runtime("./test_cases/infinite_loop", 100, 1000);
    //    let (_tx, unix_stream_rx) = create_user_rt_params_to_run();
    //    let data = user_rt.run(unix_stream_rx).await.unwrap();
    //    assert_eq!(data, EdgeCallResult::TimeOut);
    //}

    #[flaky_test]
    async fn test_unresolved_promise() {
        let user_rt = create_basic_user_runtime("./test_cases/unresolved_promise", 100, 1000).await;
        let (_tx, unix_stream_rx) = create_user_rt_params_to_run();
        let data = user_rt.run(unix_stream_rx).await.unwrap();
        assert_eq!(data, EdgeCallResult::ModuleEvaluationTimedOut);
    }

    #[flaky_test]
    async fn test_delayed_promise() {
        let user_rt =
            create_basic_user_runtime("./test_cases/resolve_promise_after_timeout", 100, 1000)
                .await;
        let (_tx, unix_stream_rx) = create_user_rt_params_to_run();
        let data = user_rt.run(unix_stream_rx).await.unwrap();
        assert_eq!(data, EdgeCallResult::TimeOut);
    }

    #[flaky_test]
    async fn test_success_delayed_promise() {
        let user_rt =
            create_basic_user_runtime("./test_cases/resolve_promise_before_timeout", 100, 1000)
                .await;
        let (_tx, unix_stream_rx) = create_user_rt_params_to_run();
        let data = user_rt.run(unix_stream_rx).await.unwrap();
        assert_eq!(data, EdgeCallResult::Completed);
    }

    #[flaky_test]
    async fn test_heap_limits_reached() {
        let user_rt = create_basic_user_runtime("./test_cases/heap_limit", 5, 1000).await;
        let (_tx, unix_stream_rx) = create_user_rt_params_to_run();
        let data = user_rt.run(unix_stream_rx).await.unwrap();
        assert_eq!(data, EdgeCallResult::HeapLimitReached);
    }

    #[flaky_test]
    async fn test_read_file_user_rt() {
        let user_rt = create_basic_user_runtime("./test_cases/readFile", 5, 1000).await;
        let (_tx, unix_stream_rx) = create_user_rt_params_to_run();
        let data = user_rt.run(unix_stream_rx).await.unwrap();
        match data {
            EdgeCallResult::ErrorThrown(data) => {
                assert!(data
                    .0
                    .to_string()
                    .contains("TypeError: Deno.readFileSync is not a function"));
            }
            _ => panic!("Invalid Result"),
        };
    }
}
