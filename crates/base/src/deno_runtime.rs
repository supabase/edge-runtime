use crate::inspector_server::Inspector;
use crate::rt_worker::rt;
use crate::rt_worker::supervisor::{CPUUsage, CPUUsageMetrics};
use crate::rt_worker::worker::DuplexStreamEntry;
use crate::utils::units::{bytes_to_display, mib_to_bytes};

use anyhow::{anyhow, bail, Context, Error};
use cooked_waker::{IntoWaker, WakeRef};
use cpu_timer::get_thread_time;
use ctor::ctor;
use deno_core::error::AnyError;
use deno_core::url::Url;
use deno_core::v8::{GCCallbackFlags, GCType, HeapStatistics, Isolate};
use deno_core::{
    located_script_name, serde_json, serde_v8, JsRuntime, ModuleCodeString, ModuleId,
    PollEventLoopOptions, RuntimeOptions,
};
use deno_http::DefaultHttpPropertyExtractor;
use deno_tls::deno_native_certs::load_native_certs;
use deno_tls::rustls;
use deno_tls::rustls::RootCertStore;
use deno_tls::RootCertStoreProvider;
use futures_util::future::poll_fn;
use futures_util::task::AtomicWaker;
use log::{error, trace};
use once_cell::sync::{Lazy, OnceCell};
use sb_core::conn_sync::ConnSync;
use sb_core::http::sb_core_http;
use sb_core::http_start::sb_core_http_start;
use sb_core::util::sync::AtomicFlag;
use serde::de::DeserializeOwned;
use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
use tokio::sync::{mpsc, watch, Notify};
use tokio::time::interval;
use tokio_util::sync::CancellationToken;

use crate::snapshot;
use event_worker::events::{EventMetadata, WorkerEventWithMetadata};
use event_worker::js_interceptors::sb_events_js_interceptors;
use event_worker::sb_user_event_worker;
use sb_ai::sb_ai;
use sb_core::cache::CacheSetting;
use sb_core::cert::ValueRootCertStoreProvider;
use sb_core::external_memory::CustomAllocator;
use sb_core::net::sb_core_net;
use sb_core::permissions::{sb_core_permissions, Permissions};
use sb_core::runtime::sb_core_runtime;
use sb_core::{sb_core_main_js, MemCheckWaker};
use sb_env::sb_env as sb_env_op;
use sb_fs::file_system::DenoCompileFileSystem;
use sb_graph::emitter::EmitterFactory;
use sb_graph::import_map::load_import_map;
use sb_graph::{
    generate_binary_eszip, include_glob_patterns_in_eszip, EszipPayloadKind, STATIC_FS_PREFIX,
};
use sb_module_loader::standalone::create_module_loader_for_standalone_from_eszip_kind;
use sb_module_loader::RuntimeProviders;
use sb_node::deno_node;
use sb_workers::context::{UserWorkerMsgs, WorkerContextInitOpts, WorkerRuntimeOpts};
use sb_workers::sb_user_workers;

const DEFAULT_ALLOC_CHECK_INT_MSEC: u64 = 1000;

static SUPABASE_UA: Lazy<String> = Lazy::new(|| {
    let deno_version = MAYBE_DENO_VERSION.get().map(|it| &**it).unwrap_or("1.0.0");
    let supabase_version = option_env!("GIT_V_TAG").unwrap_or("0.1.0");
    format!(
        "Deno/{} (variant; SupabaseEdgeRuntime/{})",
        deno_version, supabase_version
    )
});

static SHOULD_DISABLE_DEPRECATED_API_WARNING: Lazy<bool> =
    Lazy::new(|| std::env::var("DENO_NO_DEPRECATION_WARNINGS").ok().is_some());

static SHOULD_USE_VERBOSE_DEPRECATED_API_WARNING: Lazy<bool> =
    Lazy::new(|| std::env::var("DENO_VERBOSE_WARNINGS").ok().is_some());

static ALLOC_CHECK_DUR: Lazy<Duration> = Lazy::new(|| {
    std::env::var("EDGE_RUNTIME_ALLOC_CHECK_INT")
        .ok()
        .and_then(|it| it.parse::<u64>().ok().map(Duration::from_millis))
        .unwrap_or_else(|| Duration::from_millis(DEFAULT_ALLOC_CHECK_INT_MSEC))
});

pub static MAYBE_DENO_VERSION: OnceCell<String> = OnceCell::new();

#[ctor]
fn init_v8_platform() {
    set_v8_flags();

    // NOTE(denoland/deno/20495): Due to the new PKU (Memory Protection Keys)
    // feature introduced in V8 11.6, We need to initialize the V8 platform on
    // the main thread that spawns V8 isolates.
    JsRuntime::init_platform(None);
}
pub struct DenoRuntimeError(Error);

impl PartialEq for DenoRuntimeError {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_string() == other.0.to_string()
    }
}

impl fmt::Debug for DenoRuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[Js Error] {}", self.0)
    }
}

fn get_error_class_name(e: &AnyError) -> &'static str {
    sb_core::errors_rt::get_error_class_name(e).unwrap_or("Error")
}

#[derive(Default, Clone)]
struct MemCheckState {
    drop_token: CancellationToken,
    limit: Option<usize>,
    waker: Arc<AtomicWaker>,
    notify: Arc<Notify>,

    #[cfg(debug_assertions)]
    exceeded: Arc<AtomicFlag>,
}

impl Drop for MemCheckState {
    fn drop(&mut self) {
        self.drop_token.cancel();
    }
}

impl MemCheckState {
    fn check(&self, isolate: &mut Isolate) -> usize {
        let Some(limit) = self.limit else {
            return 0;
        };

        let mut stats = HeapStatistics::default();

        isolate.get_heap_statistics(&mut stats);

        // NOTE: https://stackoverflow.com/questions/41541843/nodejs-v8-getheapstatistics-method
        let malloced_bytes = stats.malloced_memory();

        // XXX(Nyannyacha): Should we instead apply a size that reflects the
        // committed heap? (but it can be bloated)
        let used_heap_bytes = stats.used_heap_size();
        let external_bytes = stats.external_memory();

        let total_bytes = malloced_bytes
            .saturating_add(used_heap_bytes)
            .saturating_add(external_bytes);

        if total_bytes >= limit {
            self.notify.notify_waiters();

            #[cfg(debug_assertions)]
            if !self.exceeded.is_raised() {
                self.exceeded.raise();
            }
        }

        total_bytes
    }

    #[allow(dead_code)]
    #[cfg(debug_assertions)]
    fn is_exceeded(&self) -> bool {
        self.exceeded.is_raised()
    }
}

pub struct DenoRuntime {
    pub js_runtime: JsRuntime,
    pub env_vars: HashMap<String, String>, // TODO: does this need to be pub?
    pub conf: WorkerRuntimeOpts,

    pub(crate) is_termination_requested: Arc<AtomicFlag>,
    pub(crate) is_terminated: Arc<AtomicFlag>,
    pub(crate) is_found_inspector_session: Arc<AtomicFlag>,

    main_module_id: ModuleId,
    maybe_inspector: Option<Inspector>,

    mem_check_state: Arc<MemCheckState>,
    waker: Arc<AtomicWaker>,
}

impl Drop for DenoRuntime {
    fn drop(&mut self) {
        if self.conf.is_user_worker() {
            self.js_runtime.v8_isolate().remove_gc_prologue_callback(
                mem_check_gc_prologue_callback_fn,
                Arc::as_ptr(&self.mem_check_state) as *mut _,
            );
        }
    }
}

impl DenoRuntime {
    #[allow(clippy::unnecessary_literal_unwrap)]
    #[allow(clippy::arc_with_non_send_sync)]
    pub async fn new(
        opts: WorkerContextInitOpts,
        maybe_inspector: Option<Inspector>,
    ) -> Result<Self, Error> {
        let WorkerContextInitOpts {
            service_path,
            no_module_cache,
            import_map_path,
            env_vars,
            events_rx,
            conf,
            maybe_eszip,
            maybe_entrypoint,
            maybe_decorator,
            maybe_module_code,
            static_patterns,
            ..
        } = opts;

        let base_dir_path = std::env::current_dir().map(|p| p.join(&service_path))?;
        let base_url = Url::from_directory_path(&base_dir_path).unwrap();

        let is_user_worker = conf.is_user_worker();

        // TODO: check for other potential main paths (eg: index.js, index.tsx)
        let mut main_module_url = base_url.join("index.ts")?;
        let is_some_entry_point = maybe_entrypoint.is_some();
        if is_some_entry_point {
            main_module_url = Url::parse(&maybe_entrypoint.unwrap())?;
        }

        let mut net_access_disabled = false;
        let mut allow_remote_modules = true;

        if is_user_worker {
            let user_conf = conf.as_user_worker().unwrap();

            net_access_disabled = user_conf.net_access_disabled;
            allow_remote_modules = user_conf.allow_remote_modules;
        }

        let mut maybe_arc_import_map = None;
        let only_module_code =
            maybe_module_code.is_some() && maybe_eszip.is_none() && !is_some_entry_point;

        let eszip = if let Some(eszip_payload) = maybe_eszip {
            eszip_payload
        } else {
            let mut emitter_factory = EmitterFactory::new();

            let cache_strategy = if no_module_cache {
                CacheSetting::ReloadAll
            } else {
                CacheSetting::Use
            };

            emitter_factory.set_file_fetcher_allow_remote(allow_remote_modules);
            emitter_factory.set_file_fetcher_cache_strategy(cache_strategy);
            emitter_factory.set_decorator_type(maybe_decorator);

            let maybe_import_map = load_import_map(import_map_path.clone())?;
            emitter_factory.set_import_map(maybe_import_map);
            maybe_arc_import_map = emitter_factory.maybe_import_map.clone();

            let arc_emitter_factory = Arc::new(emitter_factory);

            let main_module_url_file_path = main_module_url.clone().to_file_path().unwrap();

            let maybe_code = if only_module_code {
                maybe_module_code
            } else {
                None
            };

            let mut eszip = generate_binary_eszip(
                main_module_url_file_path,
                arc_emitter_factory,
                maybe_code,
                import_map_path.clone(),
            )
            .await?;

            include_glob_patterns_in_eszip(
                static_patterns.iter().map(|s| s.as_str()).collect(),
                &mut eszip,
                Some(STATIC_FS_PREFIX.to_string()),
            )
            .await;

            EszipPayloadKind::Eszip(eszip)
        };

        // Create and populate a root cert store based on environment variable.
        // Reference: https://github.com/denoland/deno/blob/v1.37.0/cli/args/mod.rs#L467
        let mut root_cert_store = RootCertStore::empty();
        let ca_stores: Vec<String> = (|| {
            let env_ca_store = std::env::var("DENO_TLS_CA_STORE").ok()?;
            Some(
                env_ca_store
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
            )
        })()
        .unwrap_or_else(|| vec!["mozilla".to_string()]);
        for store in ca_stores.iter() {
            match store.as_str() {
                "mozilla" => {
                    root_cert_store = deno_tls::create_default_root_cert_store();
                }
                "system" => {
                    let roots = load_native_certs().expect("could not load platform certs");
                    for root in roots {
                        root_cert_store
                            .add(&rustls::Certificate(root.0))
                            .expect("Failed to add platform cert to root cert store");
                    }
                }
                _ => {
                    bail!(
                        "Unknown certificate store \"{0}\" specified (allowed: \"system,mozilla\")",
                        store
                    );
                }
            }
        }

        let root_cert_store_provider: Arc<dyn RootCertStoreProvider> =
            Arc::new(ValueRootCertStoreProvider::new(root_cert_store.clone()));

        let mut stdio = Some(Default::default());
        if is_user_worker {
            stdio = Some(deno_io::Stdio {
                stdin: deno_io::StdioPipe::File(std::fs::File::create("/dev/null")?),
                stdout: deno_io::StdioPipe::File(std::fs::File::create("/dev/null")?),
                stderr: deno_io::StdioPipe::File(std::fs::File::create("/dev/null")?),
            });
        }

        let rt_provider = create_module_loader_for_standalone_from_eszip_kind(
            eszip,
            maybe_arc_import_map,
            import_map_path,
        )
        .await?;

        let RuntimeProviders {
            npm_resolver,
            vfs,
            module_loader,
            module_code,
            static_files,
            npm_snapshot,
            vfs_path,
        } = rt_provider;

        let op_fs = {
            if is_user_worker {
                Arc::new(sb_fs::static_fs::StaticFs::new(
                    static_files,
                    vfs_path,
                    vfs,
                    npm_snapshot,
                )) as Arc<dyn deno_fs::FileSystem>
            } else {
                Arc::new(DenoCompileFileSystem::from_rc(vfs)) as Arc<dyn deno_fs::FileSystem>
            }
        };

        let mod_code = module_code;

        let extensions = vec![
            sb_core_permissions::init_ops(net_access_disabled),
            deno_webidl::deno_webidl::init_ops(),
            deno_console::deno_console::init_ops(),
            deno_url::deno_url::init_ops(),
            deno_web::deno_web::init_ops::<Permissions>(
                Arc::new(deno_web::BlobStore::default()),
                None,
            ),
            deno_webgpu::deno_webgpu::init_ops(),
            deno_canvas::deno_canvas::init_ops(),
            deno_fetch::deno_fetch::init_ops::<Permissions>(deno_fetch::Options {
                user_agent: SUPABASE_UA.clone(),
                root_cert_store_provider: Some(root_cert_store_provider.clone()),
                ..Default::default()
            }),
            deno_websocket::deno_websocket::init_ops::<Permissions>(
                SUPABASE_UA.clone(),
                Some(root_cert_store_provider.clone()),
                None,
            ),
            // TODO: support providing a custom seed for crypto
            deno_crypto::deno_crypto::init_ops(None),
            deno_broadcast_channel::deno_broadcast_channel::init_ops(
                deno_broadcast_channel::InMemoryBroadcastChannel::default(),
            ),
            deno_net::deno_net::init_ops::<Permissions>(Some(root_cert_store_provider), None),
            deno_tls::deno_tls::init_ops(),
            deno_http::deno_http::init_ops::<DefaultHttpPropertyExtractor>(),
            deno_io::deno_io::init_ops(stdio),
            deno_fs::deno_fs::init_ops::<Permissions>(op_fs.clone()),
            sb_env_op::init_ops(),
            sb_ai::init_ops(),
            sb_os::sb_os::init_ops(),
            sb_user_workers::init_ops(),
            sb_user_event_worker::init_ops(),
            sb_events_js_interceptors::init_ops(),
            sb_core_main_js::init_ops(),
            sb_core_net::init_ops(),
            sb_core_http::init_ops(),
            sb_core_http_start::init_ops(),
            // NOTE(AndresP): Order is matters. Otherwise, it will lead to hard
            // errors such as SIGBUS depending on the platform.
            deno_node::init_ops::<Permissions>(Some(npm_resolver), op_fs),
            sb_core_runtime::init_ops(Some(main_module_url.clone())),
        ];

        let mut create_params = None;
        let mut mem_check_state = MemCheckState::default();

        if conf.is_user_worker() {
            let memory_limit =
                mib_to_bytes(conf.as_user_worker().unwrap().memory_limit_mb) as usize;

            let allocator = CustomAllocator::new(memory_limit);

            allocator.set_waker(mem_check_state.waker.clone());

            mem_check_state.limit = Some(memory_limit);
            create_params = Some(
                deno_core::v8::CreateParams::default()
                    .heap_limits(mib_to_bytes(0) as usize, memory_limit)
                    .array_buffer_allocator(allocator.into_v8_allocator()),
            )
        };

        let mem_check_state = Arc::new(mem_check_state);
        let runtime_options = RuntimeOptions {
            extensions,
            is_main: true,
            inspector: maybe_inspector.is_some(),
            create_params,
            get_error_class_fn: Some(&get_error_class_name),
            shared_array_buffer_store: None,
            compiled_wasm_module_store: None,
            startup_snapshot: Some(snapshot::snapshot()),
            module_loader: Some(module_loader),
            ..Default::default()
        };

        let mut js_runtime = JsRuntime::new(runtime_options);
        let version: Option<&str> = option_env!("GIT_V_TAG");

        // Bootstrapping stage
        let script = format!(
            "globalThis.bootstrapSBEdge({})",
            serde_json::json!([
                // 0: target
                env!("TARGET"),
                // 1: isUserWorker
                conf.is_user_worker(),
                // 2: isEventsWorker
                conf.is_events_worker(),
                // 3: edgeRuntimeVersion
                version.unwrap_or("0.1.0"),
                // 4: denoVersion
                MAYBE_DENO_VERSION
                    .get()
                    .map(|it| &**it)
                    .unwrap_or("UNKNOWN"),
                // 5: shouldDisableDeprecatedApiWarning
                &*SHOULD_DISABLE_DEPRECATED_API_WARNING,
                // 6: shouldUseVerboseDeprecatedApiWarning
                &*SHOULD_USE_VERBOSE_DEPRECATED_API_WARNING,
            ])
        );

        if let Some(inspector) = maybe_inspector.clone() {
            inspector.server.register_inspector(
                main_module_url.to_string(),
                &mut js_runtime,
                inspector.should_wait_for_session(),
            );
        }

        if is_user_worker {
            js_runtime.v8_isolate().add_gc_prologue_callback(
                mem_check_gc_prologue_callback_fn,
                Arc::as_ptr(&mem_check_state) as *mut _,
                GCType::ALL,
            );

            js_runtime
                .op_state()
                .borrow_mut()
                .put(MemCheckWaker::from(mem_check_state.waker.clone()));
        }

        js_runtime
            .execute_script(located_script_name!(), ModuleCodeString::from(script))
            .expect("Failed to execute bootstrap script");

        {
            // run inside a closure, so op_state_rc is released
            let op_state_rc = js_runtime.op_state();
            let mut op_state = op_state_rc.borrow_mut();

            let mut env_vars = env_vars.clone();

            if conf.is_events_worker() {
                // if worker is an events worker, assert events_rx is to be available
                op_state
                    .put::<mpsc::UnboundedReceiver<WorkerEventWithMetadata>>(events_rx.unwrap());
            }

            if conf.is_main_worker() || conf.is_user_worker() {
                op_state.put::<HashMap<usize, watch::Receiver<ConnSync>>>(HashMap::new());
            }

            if conf.is_user_worker() {
                let conf = conf.as_user_worker().unwrap();

                // set execution id for user workers
                env_vars.insert(
                    "SB_EXECUTION_ID".to_string(),
                    conf.key.map_or("".to_string(), |k| k.to_string()),
                );

                if let Some(events_msg_tx) = conf.events_msg_tx.clone() {
                    op_state.put::<mpsc::UnboundedSender<WorkerEventWithMetadata>>(events_msg_tx);
                    op_state.put::<EventMetadata>(EventMetadata {
                        service_path: conf.service_path.clone(),
                        execution_id: conf.key,
                    });
                }
            }

            op_state.put::<sb_env::EnvVars>(env_vars);
        }

        let main_module_id = js_runtime
            .load_main_module(&main_module_url, mod_code)
            .await?;

        if is_user_worker {
            drop(rt::SUPERVISOR_RT.spawn({
                let drop_token = mem_check_state.drop_token.clone();
                let waker = mem_check_state.waker.clone();

                async move {
                    // TODO(Nyannyacha): Should we introduce exponential
                    // backoff?
                    let mut int = interval(*ALLOC_CHECK_DUR);
                    loop {
                        tokio::select! {
                            _ = int.tick() => {
                                waker.wake();
                            }

                            _ = drop_token.cancelled() => {
                                break;
                            }
                        }
                    }
                }
            }));
        }

        Ok(Self {
            js_runtime,
            env_vars,
            conf,

            is_termination_requested: Arc::default(),
            is_terminated: Arc::default(),
            is_found_inspector_session: Arc::default(),

            main_module_id,
            maybe_inspector,

            mem_check_state,
            waker: Arc::default(),
        })
    }

    pub async fn run(
        &mut self,
        duplex_stream_rx: mpsc::UnboundedReceiver<DuplexStreamEntry>,
        maybe_cpu_usage_metrics_tx: Option<mpsc::UnboundedSender<CPUUsageMetrics>>,
        name: Option<String>,
    ) -> (Result<(), Error>, i64) {
        {
            let op_state_rc = self.js_runtime.op_state();
            let mut op_state = op_state_rc.borrow_mut();

            op_state.put::<mpsc::UnboundedReceiver<DuplexStreamEntry>>(duplex_stream_rx);

            if self.conf.is_main_worker() {
                op_state.put::<mpsc::UnboundedSender<UserWorkerMsgs>>(
                    self.conf.as_main_worker().unwrap().worker_pool_tx.clone(),
                );
            }
        }

        let send_cpu_metrics_fn = |metric: CPUUsageMetrics| {
            if let Some(cpu_metric_tx) = maybe_cpu_usage_metrics_tx.as_ref() {
                let _ = cpu_metric_tx.send(metric);
            }
        };

        // NOTE: This is unnecessary on the LIFO task scheduler that can't steal
        // the task from the other threads.
        let current_thread_id = std::thread::current().id();
        let mut current_cpu_time_ns;
        let mut accumulated_cpu_time_ns = 0i64;

        let inspector = self.inspector();
        let mod_result_rx = unsafe {
            self.js_runtime.v8_isolate().enter();

            if inspector.is_some() {
                {
                    let _guard = scopeguard::guard(self.is_found_inspector_session.clone(), |v| {
                        v.raise();
                    });

                    // XXX(Nyannyacha): Suppose the user skips this function by passing
                    // the `--inspect` argument. In that case, the runtime may terminate
                    // before the inspector session is connected if the function doesn't
                    // have a long execution time. Should we wait for an inspector
                    // session to connect with the V8?
                    self.wait_for_inspector_session();
                }

                if self.is_termination_requested.is_raised() {
                    self.js_runtime.v8_isolate().exit();
                    self.is_terminated.raise();
                    return (Ok(()), 0i64);
                }
            }

            let mut js_runtime = scopeguard::guard(&mut self.js_runtime, |it| {
                it.v8_isolate().exit();
            });

            send_cpu_metrics_fn(CPUUsageMetrics::Enter(current_thread_id));

            current_cpu_time_ns = get_current_cpu_time_ns().unwrap();

            let top_level_await_fut = js_runtime.mod_evaluate(self.main_module_id);
            let cpu_time_after_eval_ns = get_current_cpu_time_ns().unwrap();
            let diff_cpu_time_ns = cpu_time_after_eval_ns - current_cpu_time_ns;

            accumulated_cpu_time_ns += diff_cpu_time_ns;

            send_cpu_metrics_fn(CPUUsageMetrics::Leave(CPUUsage {
                accumulated: accumulated_cpu_time_ns,
                diff: diff_cpu_time_ns,
            }));

            top_level_await_fut
        };

        let is_termination_requested = self.is_termination_requested.clone();
        let is_user_worker = self.conf.is_user_worker();
        let global_waker = self.waker.clone();
        let mem_check_state = is_user_worker.then(|| self.mem_check_state.clone());

        let poll_result = poll_fn(|cx| unsafe {
            // INVARIANT: Only can steal current task by other threads when LIFO
            // task scheduler heuristic disabled. Turning off the heuristic is
            // unstable now, so it's not considered.
            #[cfg(debug_assertions)]
            assert_eq!(current_thread_id, std::thread::current().id());

            let waker = cx.waker();
            let woked = global_waker.take().is_none();
            let thread_id = std::thread::current().id();

            global_waker.register(waker);

            let mut js_runtime = scopeguard::guard(&mut self.js_runtime, |it| {
                it.v8_isolate().exit();
            });

            js_runtime.v8_isolate().enter();
            send_cpu_metrics_fn(CPUUsageMetrics::Enter(thread_id));

            current_cpu_time_ns = get_current_cpu_time_ns().unwrap();

            let wait_for_inspector = if inspector.is_some() {
                let inspector = js_runtime.inspector();
                let inspector_ref = inspector.borrow();
                inspector_ref.has_active_sessions() || inspector_ref.has_blocking_sessions()
            } else {
                false
            };

            let need_pool_event_loop = !is_user_worker || woked;
            let poll_result = if need_pool_event_loop {
                struct JsRuntimeWaker(Arc<AtomicWaker>);

                impl WakeRef for JsRuntimeWaker {
                    fn wake_by_ref(&self) {
                        self.0.wake();
                    }
                }

                let waker = if is_user_worker {
                    Cow::Owned(Arc::new(JsRuntimeWaker(global_waker.clone())).into_waker())
                } else {
                    Cow::Borrowed(waker)
                };

                js_runtime.poll_event_loop(
                    &mut std::task::Context::from_waker(waker.as_ref()),
                    PollEventLoopOptions {
                        wait_for_inspector,
                        pump_v8_message_loop: true,
                    },
                )
            } else {
                Poll::Pending
            };

            let cpu_time_after_poll_ns = get_current_cpu_time_ns().unwrap();
            let diff_cpu_time_ns = if need_pool_event_loop {
                cpu_time_after_poll_ns - current_cpu_time_ns
            } else {
                0
            };

            accumulated_cpu_time_ns += diff_cpu_time_ns;

            send_cpu_metrics_fn(CPUUsageMetrics::Leave(CPUUsage {
                accumulated: accumulated_cpu_time_ns,
                diff: diff_cpu_time_ns,
            }));

            if is_user_worker {
                let mem_state = mem_check_state.as_ref().unwrap();
                let total_malloced_bytes = mem_state.check(js_runtime.v8_isolate().as_mut());

                mem_state.waker.register(waker);

                trace!(
                    "name: {:?}, thread_id: {:?}, accumulated_cpu_time: {}ms, malloced: {}",
                    name.as_ref(),
                    thread_id,
                    accumulated_cpu_time_ns / 1_000_000,
                    bytes_to_display(total_malloced_bytes as u64)
                );
            }

            // NOTE(Nyannyacha): If tasks are empty or V8 is not evaluating the
            // function, and so V8 is no longer inside its loop, it turns out
            // that requesting termination does not work; thus, we need another
            // way to escape from the polling loop so the supervisor can
            // terminate the runtime.
            if need_pool_event_loop
                && poll_result.is_pending()
                && is_termination_requested.is_raised()
            {
                return Poll::Ready(Ok(()));
            }

            poll_result
        })
        .await;

        let result = match poll_result {
            Err(err) => Err(anyhow!("event loop error: {}", err)),
            Ok(_) => match mod_result_rx.await {
                Err(e) => {
                    error!("{}", e.to_string());
                    Err(e)
                }
                Ok(_) => Ok(()),
            },
        };

        self.is_terminated.raise();

        (result, accumulated_cpu_time_ns / 1_000_000)
    }

    pub fn inspector(&self) -> Option<Inspector> {
        self.maybe_inspector.clone()
    }

    pub fn add_memory_limit_callback<C>(&self, mut cb: C)
    where
        // XXX(Nyannyacha): Should we relax bounds a bit more?
        C: FnMut() -> bool + Send + 'static,
    {
        let notify = self.mem_check_state.notify.clone();
        let drop_token = self.mem_check_state.drop_token.clone();

        drop(rt::SUPERVISOR_RT.spawn(async move {
            loop {
                tokio::select! {
                    _ = notify.notified() => {
                        if cb() {
                            break;
                        }
                    }

                    _ = drop_token.cancelled() => {
                        break;
                    }
                }
            }
        }));
    }

    fn wait_for_inspector_session(&mut self) {
        if let Some(inspector) = self.maybe_inspector.as_ref() {
            let inspector_impl = self.js_runtime.inspector();
            let mut inspector_impl_ref = inspector_impl.borrow_mut();

            if inspector.option.is_with_break() {
                inspector_impl_ref.wait_for_session_and_break_on_next_statement();
            } else if inspector.option.is_with_wait() {
                inspector_impl_ref.wait_for_session();
            }
        }
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

fn get_current_cpu_time_ns() -> Result<i64, Error> {
    get_thread_time().context("can't get current thread time")
}

fn set_v8_flags() {
    let v8_flags = std::env::var("V8_FLAGS").unwrap_or("".to_string());
    let mut vec = vec![""];

    if v8_flags.is_empty() {
        return;
    }

    vec.append(&mut v8_flags.split(' ').collect());

    let ignored = deno_core::v8_set_flags(vec.iter().map(|v| v.to_string()).collect());

    if *ignored.as_slice() != [""] {
        error!("v8 flags unrecognized {:?}", ignored);
    }
}

extern "C" fn mem_check_gc_prologue_callback_fn(
    isolate: *mut Isolate,
    _ty: GCType,
    _flags: GCCallbackFlags,
    data: *mut c_void,
) {
    unsafe {
        (*(data as *mut MemCheckState)).check(&mut *isolate);
    }
}

#[cfg(test)]
mod test {
    use crate::deno_runtime::DenoRuntime;
    use crate::rt_worker::worker::DuplexStreamEntry;
    use deno_core::{FastString, ModuleCodeString, PollEventLoopOptions};
    use sb_graph::emitter::EmitterFactory;
    use sb_graph::{generate_binary_eszip, EszipPayloadKind};
    use sb_workers::context::{
        MainWorkerRuntimeOpts, UserWorkerMsgs, UserWorkerRuntimeOpts, WorkerContextInitOpts,
        WorkerRuntimeOpts,
    };
    use serial_test::serial;
    use std::collections::HashMap;
    use std::fs;
    use std::fs::File;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    #[tokio::test]
    #[serial]
    async fn test_module_code_no_eszip() {
        let (worker_pool_tx, _) = mpsc::unbounded_channel::<UserWorkerMsgs>();

        DenoRuntime::new(
            WorkerContextInitOpts {
                service_path: PathBuf::from("./test_cases/"),
                no_module_cache: false,
                import_map_path: None,
                env_vars: Default::default(),
                events_rx: None,
                timing: None,
                maybe_eszip: None,
                maybe_entrypoint: None,
                maybe_decorator: None,
                maybe_module_code: Some(FastString::from(String::from(
                    "Deno.serve((req) => new Response('Hello World'));",
                ))),
                conf: {
                    WorkerRuntimeOpts::MainWorker(MainWorkerRuntimeOpts {
                        worker_pool_tx,
                        shared_metric_src: None,
                        event_worker_metric_src: None,
                    })
                },
                static_patterns: vec![],
            },
            None,
        )
        .await
        .expect("It should not panic");
    }

    #[tokio::test]
    #[serial]
    #[allow(clippy::arc_with_non_send_sync)]
    async fn test_eszip_with_source_file() {
        let (worker_pool_tx, _) = mpsc::unbounded_channel::<UserWorkerMsgs>();
        let mut file = File::create("./test_cases/eszip-source-test.ts").unwrap();
        file.write_all(b"import isEven from \"npm:is-even\"; globalThis.isTenEven = isEven(9);")
            .unwrap();
        let path_buf = PathBuf::from("./test_cases/eszip-source-test.ts");
        let emitter_factory = Arc::new(EmitterFactory::new());
        let bin_eszip = generate_binary_eszip(path_buf, emitter_factory.clone(), None, None)
            .await
            .unwrap();
        fs::remove_file("./test_cases/eszip-source-test.ts").unwrap();

        let eszip_code = bin_eszip.into_bytes();

        let runtime = DenoRuntime::new(
            WorkerContextInitOpts {
                service_path: PathBuf::from("./test_cases/"),
                no_module_cache: false,
                import_map_path: None,
                env_vars: Default::default(),
                events_rx: None,
                timing: None,
                maybe_eszip: Some(EszipPayloadKind::VecKind(eszip_code)),
                maybe_entrypoint: None,
                maybe_decorator: None,
                maybe_module_code: None,
                conf: {
                    WorkerRuntimeOpts::MainWorker(MainWorkerRuntimeOpts {
                        worker_pool_tx,
                        shared_metric_src: None,
                        event_worker_metric_src: None,
                    })
                },
                static_patterns: vec![],
            },
            None,
        )
        .await;

        let mut rt = runtime.unwrap();
        let main_mod_ev = rt.js_runtime.mod_evaluate(rt.main_module_id);
        let _ = rt
            .js_runtime
            .run_event_loop(PollEventLoopOptions {
                wait_for_inspector: false,
                pump_v8_message_loop: true,
            })
            .await;

        let read_is_even_global = rt
            .js_runtime
            .execute_script(
                "<anon>",
                ModuleCodeString::from(
                    r#"
            globalThis.isTenEven;
        "#
                    .to_string(),
                ),
            )
            .unwrap();
        let read_is_even = rt.to_value::<deno_core::serde_json::Value>(&read_is_even_global);
        assert_eq!(read_is_even.unwrap().to_string(), "false");
        std::mem::drop(main_mod_ev);
    }

    #[tokio::test]
    #[serial]
    #[allow(clippy::arc_with_non_send_sync)]
    async fn test_create_eszip_from_graph() {
        let (worker_pool_tx, _) = mpsc::unbounded_channel::<UserWorkerMsgs>();
        let file = PathBuf::from("./test_cases/eszip-silly-test/index.ts");
        let service_path = PathBuf::from("./test_cases/eszip-silly-test");
        let emitter_factory = Arc::new(EmitterFactory::new());
        let binary_eszip = generate_binary_eszip(file, emitter_factory.clone(), None, None)
            .await
            .unwrap();

        let eszip_code = binary_eszip.into_bytes();

        let runtime = DenoRuntime::new(
            WorkerContextInitOpts {
                service_path,
                no_module_cache: false,
                import_map_path: None,
                env_vars: Default::default(),
                events_rx: None,
                timing: None,
                maybe_eszip: Some(EszipPayloadKind::VecKind(eszip_code)),
                maybe_entrypoint: None,
                maybe_decorator: None,
                maybe_module_code: None,
                conf: {
                    WorkerRuntimeOpts::MainWorker(MainWorkerRuntimeOpts {
                        worker_pool_tx,
                        shared_metric_src: None,
                        event_worker_metric_src: None,
                    })
                },
                static_patterns: vec![],
            },
            None,
        )
        .await;

        let mut rt = runtime.unwrap();
        let main_mod_ev = rt.js_runtime.mod_evaluate(rt.main_module_id);
        let _ = rt
            .js_runtime
            .run_event_loop(PollEventLoopOptions {
                wait_for_inspector: false,
                pump_v8_message_loop: true,
            })
            .await;

        let read_is_even_global = rt
            .js_runtime
            .execute_script(
                "<anon>",
                ModuleCodeString::from(
                    r#"
            globalThis.isTenEven;
        "#
                    .to_string(),
                ),
            )
            .unwrap();
        let read_is_even = rt.to_value::<deno_core::serde_json::Value>(&read_is_even_global);
        assert_eq!(read_is_even.unwrap().to_string(), "true");
        std::mem::drop(main_mod_ev);
    }

    async fn create_runtime(
        path: Option<&str>,
        env_vars: Option<HashMap<String, String>>,
        user_conf: Option<WorkerRuntimeOpts>,
        static_patterns: Vec<String>,
    ) -> DenoRuntime {
        let (worker_pool_tx, _) = mpsc::unbounded_channel::<UserWorkerMsgs>();

        DenoRuntime::new(
            WorkerContextInitOpts {
                service_path: path
                    .map(PathBuf::from)
                    .unwrap_or(PathBuf::from("./test_cases/main")),

                no_module_cache: false,
                import_map_path: None,
                env_vars: env_vars.unwrap_or_default(),
                events_rx: None,
                timing: None,
                maybe_eszip: None,
                maybe_entrypoint: None,
                maybe_decorator: None,
                maybe_module_code: None,
                conf: {
                    if let Some(uc) = user_conf {
                        uc
                    } else {
                        WorkerRuntimeOpts::MainWorker(MainWorkerRuntimeOpts {
                            worker_pool_tx,
                            shared_metric_src: None,
                            event_worker_metric_src: None,
                        })
                    }
                },
                static_patterns,
            },
            None,
        )
        .await
        .unwrap()
    }

    // Main Runtime should have access to `EdgeRuntime`
    #[tokio::test]
    #[serial]
    async fn test_main_runtime_creation() {
        let mut runtime = create_runtime(None, None, None, vec![]).await;

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
    #[serial]
    async fn test_user_runtime_creation() {
        let mut runtime = create_runtime(
            None,
            None,
            Some(WorkerRuntimeOpts::UserWorker(Default::default())),
            vec![],
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
    #[serial]
    async fn test_main_rt_fs() {
        let mut main_rt =
            create_runtime(None, Some(std::env::vars().collect()), None, vec![]).await;

        let global_value_deno_read_file_script = main_rt
            .js_runtime
            .execute_script(
                "<anon>",
                ModuleCodeString::from(
                    r#"
            Deno.readTextFileSync("./test_cases/readFile/hello_world.json");
        "#
                    .to_string(),
                ),
            )
            .unwrap();
        let fs_read_result =
            main_rt.to_value::<deno_core::serde_json::Value>(&global_value_deno_read_file_script);
        assert_eq!(
            fs_read_result.unwrap().as_str().unwrap(),
            "{\n  \"hello\": \"world\"\n}"
        );
    }

    // #[tokio::test]
    // async fn test_node_builtin_imports() {
    //     let mut main_rt = create_runtime(
    //         Some(PathBuf::from("./test_cases/node-built-in")),
    //         Some(std::env::vars().collect()),
    //         None,
    //     )
    //     .await;
    //     let mod_evaluate = main_rt.js_runtime.mod_evaluate(main_rt.main_module_id);
    //     let _ = main_rt.js_runtime.run_event_loop(false).await;
    //     let global_value_deno_read_file_script = main_rt
    //         .js_runtime
    //         .execute_script(
    //             "<anon>",
    //             r#"
    //         globalThis.basename('/Users/Refsnes/demo_path.js');
    //     "#,
    //         )
    //         .unwrap();
    //     let fs_read_result =
    //         main_rt.to_value::<deno_core::serde_json::Value>(&global_value_deno_read_file_script);
    //     assert_eq!(fs_read_result.unwrap().as_str().unwrap(), "demo_path.js");
    //     std::mem::drop(mod_evaluate);
    // }

    #[tokio::test]
    #[serial]
    async fn test_static_fs() {
        let mut user_rt = create_runtime(
            None,
            None,
            Some(WorkerRuntimeOpts::UserWorker(Default::default())),
            vec![String::from("./test_cases/**/*.md")],
        )
        .await;

        let user_rt_execute_scripts = user_rt
            .js_runtime
            .execute_script(
                "<anon>",
                ModuleCodeString::from(
                    r#"Deno.readTextFileSync("./mnt/data/test_cases/content.md")"#.to_string(),
                ),
            )
            .unwrap();
        let serde_deno_env = user_rt
            .to_value::<deno_core::serde_json::Value>(&user_rt_execute_scripts)
            .unwrap();

        assert_eq!(
            serde_deno_env,
            deno_core::serde_json::Value::String(String::from("Some test file"))
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_os_ops() {
        let mut user_rt = create_runtime(
            None,
            None,
            Some(WorkerRuntimeOpts::UserWorker(Default::default())),
            vec![],
        )
        .await;

        let user_rt_execute_scripts = user_rt
            .js_runtime
            .execute_script(
                "<anon>",
                ModuleCodeString::from(
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
        "#
                    .to_string(),
                ),
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
        assert_eq!(loadavg_array.first().unwrap().as_f64().unwrap(), 0.0);
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
        assert_eq!(
            deno_version_array.first().unwrap().as_str().unwrap(),
            "supabase-edge-runtime-0.1.0 (compatible with Deno vUNKNOWN)"
        );
        assert_eq!(
            deno_version_array.get(1).unwrap().as_str().unwrap(),
            "11.6.189.12"
        );
        assert_eq!(
            deno_version_array.get(2).unwrap().as_str().unwrap(),
            "5.1.6"
        );

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
            ModuleCodeString::from(
                r#"
            let cmd = new Deno.Command("", {});
            cmd.outputSync();
        "#
                .to_string(),
            ),
        );
        assert!(user_rt_execute_scripts.is_err());
        assert!(user_rt_execute_scripts
            .unwrap_err()
            .to_string()
            .contains("Spawning subprocesses is not allowed on Supabase Edge Runtime"));
    }

    #[tokio::test]
    #[serial]
    async fn test_os_env_vars() {
        std::env::set_var("Supa_Test", "Supa_Value");
        let mut main_rt =
            create_runtime(None, Some(std::env::vars().collect()), None, vec![]).await;
        let mut user_rt = create_runtime(
            None,
            None,
            Some(WorkerRuntimeOpts::UserWorker(Default::default())),
            vec![],
        )
        .await;
        assert!(!main_rt.env_vars.is_empty());
        assert!(user_rt.env_vars.is_empty());

        let err = main_rt
            .js_runtime
            .execute_script(
                "<anon>",
                ModuleCodeString::from(
                    r#"
            // Should not be able to set
            Deno.env.set("Supa_Test", "Supa_Value");
        "#
                    .to_string(),
                ),
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
                ModuleCodeString::from(
                    r#"
            // Should not be able to set
            Deno.env.get("Supa_Test");
        "#
                    .to_string(),
                ),
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
                ModuleCodeString::from(
                    r#"
            // Should not be able to set
            Deno.env.get("Supa_Test");
        "#
                    .to_string(),
                ),
            )
            .unwrap();
        let user_serde_deno_env =
            user_rt.to_value::<deno_core::serde_json::Value>(&user_deno_env_get_supa_test);
        assert!(user_serde_deno_env.unwrap().is_null());
    }

    async fn create_basic_user_runtime(
        path: &str,
        memory_limit_mb: u64,
        worker_timeout_ms: u64,
        static_patterns: &[&str],
    ) -> DenoRuntime {
        create_runtime(
            Some(path),
            None,
            Some(WorkerRuntimeOpts::UserWorker(UserWorkerRuntimeOpts {
                memory_limit_mb,
                worker_timeout_ms,
                cpu_time_soft_limit_ms: 100,
                cpu_time_hard_limit_ms: 200,
                force_create: true,
                ..Default::default()
            })),
            static_patterns.iter().map(|it| String::from(*it)).collect(),
        )
        .await
    }

    #[tokio::test]
    #[serial]
    async fn test_array_buffer_allocation_below_limit() {
        let mut user_rt =
            create_basic_user_runtime("./test_cases/array_buffers", 20, 1000, &[]).await;

        let (_tx, duplex_stream_rx) = mpsc::unbounded_channel::<DuplexStreamEntry>();
        let (result, _) = user_rt.run(duplex_stream_rx, None, None).await;

        assert!(result.is_ok(), "expected no errors");

        // however, mem checker must be raised because it aggregates heap usage
        assert!(user_rt.mem_check_state.is_exceeded());
    }

    #[tokio::test]
    #[serial]
    async fn test_array_buffer_allocation_above_limit() {
        let mut user_rt =
            create_basic_user_runtime("./test_cases/array_buffers", 15, 1000, &[]).await;

        let (_tx, duplex_stream_rx) = mpsc::unbounded_channel::<DuplexStreamEntry>();
        let (result, _) = user_rt.run(duplex_stream_rx, None, None).await;

        match result {
            Err(err) => {
                assert!(err
                    .to_string()
                    .contains("RangeError: Array buffer allocation failed"));
            }
            _ => panic!("Invalid Result"),
        };
    }

    async fn test_mem_check_above_limit(
        path: &str,
        static_patterns: &[&str],
        memory_limit_mb: u64,
        worker_timeout_ms: u64,
    ) {
        let (_duplex_stream_tx, duplex_stream_rx) = mpsc::unbounded_channel::<DuplexStreamEntry>();
        let (callback_tx, mut callback_rx) = mpsc::unbounded_channel::<()>();
        let mut user_rt =
            create_basic_user_runtime(path, memory_limit_mb, worker_timeout_ms, static_patterns)
                .await;

        let waker = user_rt.js_runtime.op_state().borrow().waker.clone();
        let handle = user_rt.js_runtime.v8_isolate().thread_safe_handle();

        user_rt.add_memory_limit_callback(move || {
            handle.terminate_execution();
            waker.wake();
            callback_tx.send(()).unwrap();
            true
        });

        let wait_fut = async move {
            let (result, _) = user_rt.run(duplex_stream_rx, None, None).await;

            assert_eq!(
                result.unwrap_err().to_string(),
                "event loop error: Uncaught Error: execution terminated"
            );

            callback_rx.recv().await.unwrap();

            assert!(user_rt.mem_check_state.is_exceeded());
        };

        if timeout(Duration::from_secs(10), wait_fut).await.is_err() {
            panic!("failed to detect a memory limit callback invocation within the given time");
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_mem_checker_above_limit_read_file_sync_api() {
        test_mem_check_above_limit(
            "./test_cases/read_file_sync_20mib",
            &["./test_cases/**/*.bin"],
            15, // 15728640 bytes
            1000,
        )
        .await;
    }

    #[tokio::test]
    #[serial]
    async fn test_mem_checker_above_limit_wasm() {
        test_mem_check_above_limit(
            "./test_cases/wasm/grow_20mib",
            &["./test_cases/**/*.wasm"],
            60, // 62914560 bytes
            1000,
        )
        .await;
    }

    #[tokio::test]
    #[serial]
    async fn test_mem_checker_above_limit_wasm_heap() {
        test_mem_check_above_limit(
            "./test_cases/wasm/heap",
            &["./test_cases/**/*.wasm"],
            60, // 62914560 bytes
            1000,
        )
        .await;
    }

    #[tokio::test]
    #[serial]
    async fn test_mem_checker_above_limit_wasm_grow_jsapi() {
        test_mem_check_above_limit(
            "./test_cases/wasm/grow_jsapi",
            &[],
            62, // 65011712 bytes < 65536000 bytes (1000 pages)
            1000,
        )
        .await;
    }

    #[tokio::test]
    #[serial]
    async fn test_mem_checker_above_limit_wasm_grow_standalone() {
        test_mem_check_above_limit(
            "./test_cases/wasm/grow_standalone",
            &["./test_cases/**/*.wasm"],
            22, // 23068672 bytes
            1000,
        )
        .await;
    }
}
