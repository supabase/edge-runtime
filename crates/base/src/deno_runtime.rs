use crate::inspector_server::Inspector;
use crate::rt_worker::supervisor::{CPUUsage, CPUUsageMetrics};
use crate::rt_worker::worker::DuplexStreamEntry;
use crate::utils::units::{bytes_to_display, mib_to_bytes};

use anyhow::{anyhow, bail, Context, Error};
use base_mem_check::{MemCheckState, WorkerHeapStatistics};
use cooked_waker::{IntoWaker, WakeRef};
use cpu_timer::get_thread_time;
use ctor::ctor;
use deno_core::error::AnyError;
use deno_core::url::Url;
use deno_core::v8::{GCCallbackFlags, GCType, HeapStatistics, Isolate};
use deno_core::{
    located_script_name, serde_json, JsRuntime, ModuleCodeString, ModuleId, PollEventLoopOptions,
    RuntimeOptions,
};
use deno_http::DefaultHttpPropertyExtractor;
use deno_tls::deno_native_certs::load_native_certs;
use deno_tls::rustls::RootCertStore;
use deno_tls::RootCertStoreProvider;
use futures_util::future::poll_fn;
use futures_util::task::AtomicWaker;
use log::{error, trace};
use once_cell::sync::{Lazy, OnceCell};
use sb_core::conn_sync::DenoRuntimeDropToken;
use sb_core::http::sb_core_http;
use sb_core::http_start::sb_core_http_start;
use sb_core::util::sync::AtomicFlag;
use sb_fs::static_fs::StaticFs;
use serde::Serialize;
use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::task::Poll;
use std::thread::ThreadId;
use std::time::Duration;
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tokio::time::interval;
use tokio_util::sync::{CancellationToken, PollSemaphore};
use tracing::debug;

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
use sb_graph::{generate_binary_eszip, include_glob_patterns_in_eszip, EszipPayloadKind};
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
        // TODO: It should be changed to a well-known name for the ecosystem.
        "Deno/{} (variant; SupabaseEdgeRuntime/{})",
        deno_version, supabase_version
    )
});

static ALLOC_CHECK_DUR: Lazy<Duration> = Lazy::new(|| {
    std::env::var("EDGE_RUNTIME_ALLOC_CHECK_INT")
        .ok()
        .and_then(|it| it.parse::<u64>().ok().map(Duration::from_millis))
        .unwrap_or_else(|| Duration::from_millis(DEFAULT_ALLOC_CHECK_INT_MSEC))
});

// Following static variables are initialized in the cli crate.

pub static SHOULD_DISABLE_DEPRECATED_API_WARNING: OnceCell<bool> = OnceCell::new();
pub static SHOULD_USE_VERBOSE_DEPRECATED_API_WARNING: OnceCell<bool> = OnceCell::new();
pub static SHOULD_INCLUDE_MALLOCED_MEMORY_ON_MEMCHECK: OnceCell<bool> = OnceCell::new();
pub static MAYBE_DENO_VERSION: OnceCell<String> = OnceCell::new();

thread_local! {
    // NOTE: Suppose we have met `.await` points while initializing a
    // DenoRuntime. In that case, the current v8 isolate's thread-local state can be
    // corrupted by a task initializing another DenoRuntime, so we must prevent this
    // with a Semaphore.

    static RUNTIME_CREATION_SEM: Arc<Semaphore> = Arc::new(Semaphore::new(1));
}

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

#[derive(Default)]
struct MemCheck {
    exceeded_token: CancellationToken,
    limit: Option<usize>,
    waker: Arc<AtomicWaker>,
    state: Arc<RwLock<MemCheckState>>,
}

impl MemCheck {
    fn check(&self, isolate: &mut Isolate) -> usize {
        let Some(limit) = self.limit else {
            return 0;
        };

        let mut stats = HeapStatistics::default();

        isolate.get_heap_statistics(&mut stats);

        // NOTE: https://stackoverflow.com/questions/41541843/nodejs-v8-getheapstatistics-method
        let malloced_bytes = if SHOULD_INCLUDE_MALLOCED_MEMORY_ON_MEMCHECK
            .get()
            .copied()
            .unwrap_or_default()
        {
            stats.malloced_memory()
        } else {
            0
        };

        // XXX(Nyannyacha): Should we instead apply a size that reflects the
        // committed heap? (but it can be bloated)
        let used_heap_bytes = stats.used_heap_size();
        let external_bytes = stats.external_memory();

        let total_bytes = malloced_bytes
            .saturating_add(used_heap_bytes)
            .saturating_add(external_bytes);

        let heap_stats = WorkerHeapStatistics::from(&stats);
        let mut state = self.state.write().unwrap();

        if !state.exceeded {
            state.current = heap_stats;

            if total_bytes >= limit {
                state.exceeded = true;

                drop(state);
                self.exceeded_token.cancel();
            }
        }

        total_bytes
    }
}

pub trait GetRuntimeContext {
    fn get_runtime_context() -> impl Serialize;
}

impl GetRuntimeContext for () {
    fn get_runtime_context() -> impl Serialize {
        serde_json::json!(null)
    }
}

pub struct DenoRuntime<RuntimeContext = ()> {
    pub drop_token: CancellationToken,
    pub js_runtime: JsRuntime,
    pub env_vars: HashMap<String, String>, // TODO: does this need to be pub?
    pub conf: WorkerRuntimeOpts,

    pub(crate) termination_request_token: CancellationToken,

    pub(crate) is_terminated: Arc<AtomicFlag>,
    pub(crate) is_found_inspector_session: Arc<AtomicFlag>,

    main_module_id: ModuleId,
    maybe_inspector: Option<Inspector>,

    mem_check: Arc<MemCheck>,
    waker: Arc<AtomicWaker>,

    _phantom_runtime_context: PhantomData<RuntimeContext>,
}

impl<RuntimeContext> Drop for DenoRuntime<RuntimeContext> {
    fn drop(&mut self) {
        self.drop_token.cancel();

        if self.conf.is_user_worker() {
            self.js_runtime.v8_isolate().remove_gc_prologue_callback(
                mem_check_gc_prologue_callback_fn,
                Arc::as_ptr(&self.mem_check) as *mut _,
            );
        }
    }
}

impl DenoRuntime<()> {
    pub async fn acquire() -> OwnedSemaphorePermit {
        RUNTIME_CREATION_SEM
            .with(|v| v.clone())
            .acquire_owned()
            .await
            .unwrap()
    }
}

impl<RuntimeContext> DenoRuntime<RuntimeContext>
where
    RuntimeContext: GetRuntimeContext,
{
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
            maybe_jsx_import_source_config,
            ..
        } = opts;

        // TODO(Nyannyacha): Make sure `service_path` is an absolute path first.

        let drop_token = CancellationToken::default();

        let base_dir_path = std::env::current_dir().map(|p| p.join(&service_path))?;
        let base_url = Url::from_directory_path(&base_dir_path).unwrap();

        let is_user_worker = conf.is_user_worker();

        let potential_exts = vec!["ts", "tsx", "js", "jsx"];
        let mut main_module_url = base_url.join("index.ts")?;

        for potential_ext in potential_exts {
            main_module_url = base_url.join(format!("index.{}", potential_ext).as_str())?;
            if main_module_url.to_file_path().unwrap().exists() {
                break;
            }
        }

        let is_some_entry_point = maybe_entrypoint.is_some();
        if is_some_entry_point {
            main_module_url = Url::parse(&maybe_entrypoint.unwrap())?;
        }

        let mut net_access_disabled = false;
        let mut allow_net = None;
        let mut allow_remote_modules = true;
        if is_user_worker {
            let user_conf = conf.as_user_worker().unwrap();

            net_access_disabled = user_conf.net_access_disabled;
            allow_remote_modules = user_conf.allow_remote_modules;

            allow_net = match &user_conf.allow_net {
                Some(allow_net) => Some(
                    allow_net
                        .iter()
                        .map(|s| FromStr::from_str(s.as_str()))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                None => None,
            };
        }

        let mut maybe_import_map = None;
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

            if let Some(jsx_import_source_config) = maybe_jsx_import_source_config.clone() {
                emitter_factory
                    .set_jsx_import_source(jsx_import_source_config)
                    .await;
            }

            emitter_factory.set_import_map(load_import_map(import_map_path.clone())?);
            maybe_import_map.clone_from(&emitter_factory.maybe_import_map);

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
                // here we don't want to add extra cost, so we won't use a checksum
                None,
            )
            .await?;

            include_glob_patterns_in_eszip(
                static_patterns.iter().map(|s| s.as_str()).collect(),
                &mut eszip,
                &base_dir_path,
            )
            .await?;

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
                            .add((&*root.0).into())
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
                stdin: deno_io::StdioPipe::file(std::fs::File::create("/dev/null")?),
                stdout: deno_io::StdioPipe::file(std::fs::File::create("/dev/null")?),
                stderr: deno_io::StdioPipe::file(std::fs::File::create("/dev/null")?),
            });
        }

        let has_inspector = maybe_inspector.is_some();
        let rt_provider = create_module_loader_for_standalone_from_eszip_kind(
            eszip,
            base_dir_path.clone(),
            maybe_import_map,
            import_map_path,
            has_inspector,
        )
        .await?;

        let RuntimeProviders {
            node_resolver,
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
                Arc::new(StaticFs::new(
                    static_files,
                    base_dir_path,
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
            sb_core_permissions::init_ops(net_access_disabled, allow_net),
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
            deno_node::init_ops::<Permissions>(Some(node_resolver), Some(npm_resolver), op_fs),
            sb_core_runtime::init_ops(Some(main_module_url.clone())),
        ];

        let mut create_params = None;
        let mut mem_check = MemCheck::default();

        if conf.is_user_worker() {
            let memory_limit =
                mib_to_bytes(conf.as_user_worker().unwrap().memory_limit_mb) as usize;

            let allocator = CustomAllocator::new(memory_limit);

            allocator.set_waker(mem_check.waker.clone());

            mem_check.limit = Some(memory_limit);
            create_params = Some(
                deno_core::v8::CreateParams::default()
                    .heap_limits(mib_to_bytes(0) as usize, memory_limit)
                    .array_buffer_allocator(allocator.into_v8_allocator()),
            )
        };

        let mem_check = Arc::new(mem_check);
        let runtime_options = RuntimeOptions {
            extensions,
            is_main: true,
            inspector: maybe_inspector.is_some(),
            create_params,
            get_error_class_fn: Some(&get_error_class_name),
            shared_array_buffer_store: None,
            compiled_wasm_module_store: None,
            startup_snapshot: snapshot::snapshot(),
            module_loader: Some(module_loader),
            ..Default::default()
        };

        let mut js_runtime = JsRuntime::new(runtime_options);
        let version: Option<&str> = option_env!("GIT_V_TAG");

        {
            // @andreespirela : We do this because "NODE_DEBUG" is trying to be read during
            // initialization, But we need the gotham state to be up-to-date
            let op_state_rc = js_runtime.op_state();
            let mut op_state = op_state_rc.borrow_mut();
            op_state.put::<sb_env::EnvVars>(sb_env::EnvVars::new());
        }

        // Bootstrapping stage
        let script = format!(
            "globalThis.bootstrapSBEdge({}, {})",
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
                SHOULD_DISABLE_DEPRECATED_API_WARNING
                    .get()
                    .copied()
                    .unwrap_or_default(),
                // 6: shouldUseVerboseDeprecatedApiWarning
                SHOULD_USE_VERBOSE_DEPRECATED_API_WARNING
                    .get()
                    .copied()
                    .unwrap_or_default(),
            ]),
            serde_json::json!(RuntimeContext::get_runtime_context())
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
                Arc::as_ptr(&mem_check) as *mut _,
                GCType::ALL,
            );

            js_runtime
                .op_state()
                .borrow_mut()
                .put(MemCheckWaker::from(mem_check.waker.clone()));
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
                op_state.put::<HashMap<usize, CancellationToken>>(HashMap::new());
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
            op_state.put(DenoRuntimeDropToken(drop_token.clone()))
        }

        let main_module_id = {
            if let Some(code) = mod_code {
                js_runtime
                    .load_main_es_module_from_code(&main_module_url, code)
                    .await?
            } else {
                js_runtime.load_main_es_module(&main_module_url).await?
            }
        };

        if is_user_worker {
            drop(base_rt::SUPERVISOR_RT.spawn({
                let drop_token = drop_token.clone();
                let waker = mem_check.waker.clone();

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
            drop_token,
            js_runtime,
            env_vars,
            conf,

            termination_request_token: CancellationToken::new(),

            is_terminated: Arc::default(),
            is_found_inspector_session: Arc::default(),

            main_module_id,
            maybe_inspector,

            mem_check,
            waker: Arc::default(),

            _phantom_runtime_context: PhantomData,
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

        let _terminate_guard = scopeguard::guard(self.is_terminated.clone(), |v| {
            v.raise();
        });

        // NOTE: This is unnecessary on the LIFO task scheduler that can't steal
        // the task from the other threads.
        let current_thread_id = std::thread::current().id();
        let mut accumulated_cpu_time_ns = 0i64;

        let has_inspector = self.inspector().is_some();
        let mut mod_result_rx = unsafe {
            self.js_runtime.v8_isolate().enter();

            if has_inspector {
                let is_terminated = self.is_terminated.clone();
                let mut this = scopeguard::guard_on_unwind(&mut *self, |this| {
                    this.js_runtime.v8_isolate().exit();
                    is_terminated.raise();
                });

                {
                    let _guard = scopeguard::guard(this.is_found_inspector_session.clone(), |v| {
                        v.raise();
                    });

                    // XXX(Nyannyacha): Suppose the user skips this function by passing
                    // the `--inspect` argument. In that case, the runtime may terminate
                    // before the inspector session is connected if the function doesn't
                    // have a long execution time. Should we wait for an inspector
                    // session to connect with the V8?
                    this.wait_for_inspector_session();
                }

                if this.termination_request_token.is_cancelled() {
                    this.js_runtime.v8_isolate().exit();
                    is_terminated.raise();
                    return (Ok(()), 0i64);
                }
            }

            let mut js_runtime = scopeguard::guard(&mut self.js_runtime, |it| {
                it.v8_isolate().exit();
            });

            with_cpu_metrics_guard(
                current_thread_id,
                &maybe_cpu_usage_metrics_tx,
                &mut accumulated_cpu_time_ns,
                || js_runtime.mod_evaluate(self.main_module_id),
            )
        };

        macro_rules! get_accumulated_cpu_time_ms {
            () => {
                accumulated_cpu_time_ns / 1_000_000
            };
        }

        {
            let event_loop_fut = self.run_event_loop(
                name.as_deref(),
                current_thread_id,
                &maybe_cpu_usage_metrics_tx,
                &mut accumulated_cpu_time_ns,
            );

            let mod_result = tokio::select! {
                // Not using biased mode leads to non-determinism for relatively simple
                // programs.
                biased;

                maybe_mod_result = &mut mod_result_rx => {
                    debug!("received module evaluate {:#?}", maybe_mod_result);
                    maybe_mod_result

                }

                event_loop_result = event_loop_fut => {
                    if let Err(err) = event_loop_result {
                        Err(anyhow!("event loop error while evaluating the module: {}", err))
                    } else {
                        mod_result_rx.await
                    }
                }
            };

            if let Err(err) = mod_result {
                return (Err(err), get_accumulated_cpu_time_ms!());
            }
        }

        if let Err(err) = self
            .run_event_loop(
                name.as_deref(),
                current_thread_id,
                &maybe_cpu_usage_metrics_tx,
                &mut accumulated_cpu_time_ns,
            )
            .await
        {
            return (
                Err(anyhow!("event loop error: {}", err)),
                get_accumulated_cpu_time_ms!(),
            );
        }

        (Ok(()), get_accumulated_cpu_time_ms!())
    }

    fn run_event_loop<'l>(
        &'l mut self,
        name: Option<&'l str>,
        #[allow(unused_variables)] current_thread_id: ThreadId,
        maybe_cpu_usage_metrics_tx: &'l Option<mpsc::UnboundedSender<CPUUsageMetrics>>,
        accumulated_cpu_time_ns: &'l mut i64,
    ) -> impl Future<Output = Result<(), AnyError>> + 'l {
        let has_inspector = self.inspector().is_some();
        let is_user_worker = self.conf.is_user_worker();
        let global_waker = self.waker.clone();
        let termination_request_token = self.termination_request_token.clone();

        let mem_check_state = is_user_worker.then(|| self.mem_check.clone());
        let mut poll_sem = None::<PollSemaphore>;

        poll_fn(move |cx| {
            if poll_sem.is_none() {
                poll_sem = Some(RUNTIME_CREATION_SEM.with(|v| PollSemaphore::new(v.clone())));
            }

            let Poll::Ready(Some(_permit)) = poll_sem.as_mut().unwrap().poll_acquire(cx) else {
                return Poll::Pending;
            };

            poll_sem = None;

            // INVARIANT: Only can steal current task by other threads when LIFO
            // task scheduler heuristic disabled. Turning off the heuristic is
            // unstable now, so it's not considered.
            #[cfg(debug_assertions)]
            assert_eq!(current_thread_id, std::thread::current().id());

            let waker = cx.waker();
            let woked = global_waker.take().is_none();
            let thread_id = std::thread::current().id();

            global_waker.register(waker);

            let mut this = self.get_v8_tls_guard();

            let js_runtime = &mut this.js_runtime;
            let cpu_metrics_guard = get_cpu_metrics_guard(
                thread_id,
                maybe_cpu_usage_metrics_tx,
                accumulated_cpu_time_ns,
            );

            let wait_for_inspector = if has_inspector {
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
                        ..Default::default()
                    },
                )
            } else {
                Poll::Pending
            };

            drop(cpu_metrics_guard);

            if is_user_worker {
                let mem_state = mem_check_state.as_ref().unwrap();
                let total_malloced_bytes = mem_state.check(js_runtime.v8_isolate().as_mut());

                mem_state.waker.register(waker);

                trace!(
                    "name: {:?}, thread_id: {:?}, accumulated_cpu_time: {}ms, malloced: {}",
                    name.as_ref(),
                    thread_id,
                    *accumulated_cpu_time_ns / 1_000_000,
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
                && termination_request_token.is_cancelled()
            {
                return Poll::Ready(Ok(()));
            }

            poll_result
        })
    }

    pub fn inspector(&self) -> Option<Inspector> {
        self.maybe_inspector.clone()
    }

    pub fn mem_check_state(&self) -> Arc<RwLock<MemCheckState>> {
        self.mem_check.state.clone()
    }

    pub fn add_memory_limit_callback<C>(&self, cb: C)
    where
        // XXX(Nyannyacha): Should we relax bounds a bit more?
        C: FnOnce(MemCheckState) + Send + 'static,
    {
        let runtime_token = self.drop_token.clone();
        let exceeded_token = self.mem_check.exceeded_token.clone();
        let state = self.mem_check_state();

        drop(base_rt::SUPERVISOR_RT.spawn(async move {
            tokio::select! {
                _ = runtime_token.cancelled_owned() => {}
                _ = exceeded_token.cancelled_owned() => {
                    let state = tokio::task::spawn_blocking({
                        let state = state.clone();
                        move || {
                            *state.read().unwrap()
                        }
                    }).await.unwrap();

                    cb(state);
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

    fn get_v8_tls_guard<'l>(
        &'l mut self,
    ) -> scopeguard::ScopeGuard<
        &'l mut DenoRuntime<RuntimeContext>,
        impl FnOnce(&'l mut DenoRuntime<RuntimeContext>) + 'l,
    > {
        let mut guard = scopeguard::guard(self, |v| unsafe {
            v.js_runtime.v8_isolate().exit();
        });

        unsafe {
            guard.js_runtime.v8_isolate().enter();
        }

        guard
    }
}

fn get_current_cpu_time_ns() -> Result<i64, Error> {
    get_thread_time().context("can't get current thread time")
}

fn with_cpu_metrics_guard<'l, F, R>(
    thread_id: ThreadId,
    maybe_cpu_usage_metrics_tx: &'l Option<mpsc::UnboundedSender<CPUUsageMetrics>>,
    accumulated_cpu_time_ns: &'l mut i64,
    work_fn: F,
) -> R
where
    F: FnOnce() -> R,
{
    let _cpu_metrics_guard = get_cpu_metrics_guard(
        thread_id,
        maybe_cpu_usage_metrics_tx,
        accumulated_cpu_time_ns,
    );

    work_fn()
}

fn get_cpu_metrics_guard<'l>(
    thread_id: ThreadId,
    maybe_cpu_usage_metrics_tx: &'l Option<mpsc::UnboundedSender<CPUUsageMetrics>>,
    accumulated_cpu_time_ns: &'l mut i64,
) -> scopeguard::ScopeGuard<(), impl FnOnce(()) + 'l> {
    let send_cpu_metrics_fn = move |metric: CPUUsageMetrics| {
        if let Some(cpu_metric_tx) = maybe_cpu_usage_metrics_tx.as_ref() {
            let _ = cpu_metric_tx.send(metric);
        }
    };

    send_cpu_metrics_fn(CPUUsageMetrics::Enter(thread_id));

    let current_cpu_time_ns = get_current_cpu_time_ns().unwrap();

    scopeguard::guard((), move |_| {
        debug_assert_eq!(thread_id, std::thread::current().id());

        let cpu_time_after_drop_ns = get_current_cpu_time_ns().unwrap_or(current_cpu_time_ns);
        let diff_cpu_time_ns = cpu_time_after_drop_ns - current_cpu_time_ns;

        *accumulated_cpu_time_ns += diff_cpu_time_ns;

        send_cpu_metrics_fn(CPUUsageMetrics::Leave(CPUUsage {
            accumulated: *accumulated_cpu_time_ns,
            diff: diff_cpu_time_ns,
        }));
    })
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
        (*(data as *mut MemCheck)).check(&mut *isolate);
    }
}

#[cfg(test)]
mod test {
    use crate::deno_runtime::DenoRuntime;
    use crate::rt_worker::worker::DuplexStreamEntry;
    use anyhow::Context;
    use deno_config::JsxImportSourceConfig;
    use deno_core::error::AnyError;
    use deno_core::{serde_json, serde_v8, v8, FastString, ModuleCodeString, PollEventLoopOptions};
    use sb_graph::emitter::EmitterFactory;
    use sb_graph::{generate_binary_eszip, EszipPayloadKind};
    use sb_workers::context::{
        MainWorkerRuntimeOpts, UserWorkerMsgs, UserWorkerRuntimeOpts, WorkerContextInitOpts,
        WorkerRuntimeOpts,
    };
    use serde::de::DeserializeOwned;
    use serde::Serialize;
    use serial_test::serial;
    use std::collections::HashMap;
    use std::fs;
    use std::fs::File;
    use std::io::Write;
    use std::marker::PhantomData;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio::time::timeout;
    use url::Url;

    use super::GetRuntimeContext;

    impl<RuntimeContext> DenoRuntime<RuntimeContext> {
        fn to_value_mut<T>(&mut self, global_value: &v8::Global<v8::Value>) -> Result<T, AnyError>
        where
            T: DeserializeOwned + 'static,
        {
            let scope = &mut self.js_runtime.handle_scope();
            let value = v8::Local::new(scope, global_value.clone());
            Ok(serde_v8::from_v8(scope, value)?)
        }
    }

    #[derive(Debug, Default)]
    struct RuntimeBuilder<C = ()> {
        path: Option<String>,
        eszip: Option<EszipPayloadKind>,
        env_vars: Option<HashMap<String, String>>,
        worker_runtime_conf: Option<WorkerRuntimeOpts>,
        static_patterns: Vec<String>,
        jsx_import_source_config: Option<JsxImportSourceConfig>,
        _phantom_context: PhantomData<C>,
    }

    impl RuntimeBuilder {
        fn new() -> Self {
            Self::default()
        }
    }

    impl<C> RuntimeBuilder<C> {
        fn set_context<C2>(self) -> RuntimeBuilder<C2>
        where
            C2: GetRuntimeContext,
        {
            RuntimeBuilder {
                path: self.path,
                eszip: self.eszip,
                env_vars: self.env_vars,
                worker_runtime_conf: self.worker_runtime_conf,
                static_patterns: self.static_patterns,
                jsx_import_source_config: self.jsx_import_source_config,
                _phantom_context: PhantomData,
            }
        }
    }

    impl<C> RuntimeBuilder<C>
    where
        C: GetRuntimeContext,
    {
        async fn build(self) -> DenoRuntime<C> {
            let RuntimeBuilder {
                path,
                eszip,
                env_vars,
                worker_runtime_conf,
                static_patterns,
                jsx_import_source_config,
                _phantom_context,
            } = self;

            let (worker_pool_tx, _) = mpsc::unbounded_channel::<UserWorkerMsgs>();

            DenoRuntime::new(
                WorkerContextInitOpts {
                    maybe_eszip: eszip,
                    service_path: path
                        .map(PathBuf::from)
                        .unwrap_or(PathBuf::from("./test_cases/main")),

                    conf: {
                        if let Some(conf) = worker_runtime_conf {
                            conf
                        } else {
                            WorkerRuntimeOpts::MainWorker(MainWorkerRuntimeOpts {
                                worker_pool_tx,
                                shared_metric_src: None,
                                event_worker_metric_src: None,
                            })
                        }
                    },

                    maybe_entrypoint: None,
                    maybe_decorator: None,
                    maybe_module_code: None,

                    no_module_cache: false,
                    env_vars: env_vars.unwrap_or_default(),

                    static_patterns,
                    maybe_jsx_import_source_config: jsx_import_source_config,

                    events_rx: None,
                    timing: None,

                    import_map_path: None,
                },
                None,
            )
            .await
            .unwrap()
        }
    }

    impl<C> RuntimeBuilder<C> {
        fn set_path(mut self, path: &str) -> Self {
            let _ = self.path.insert(path.to_string());
            self
        }

        async fn set_eszip<P>(mut self, path: P) -> Result<Self, anyhow::Error>
        where
            P: AsRef<Path>,
        {
            let _ = self.eszip.insert(EszipPayloadKind::VecKind(
                tokio::fs::read(path)
                    .await
                    .context("cannot read eszip binary")?,
            ));

            Ok(self)
        }

        fn set_env_vars(mut self, vars: HashMap<String, String>) -> Self {
            let _ = self.env_vars.insert(vars);
            self
        }

        fn set_std_env(self) -> Self {
            self.set_env_vars(std::env::vars().collect())
        }

        fn set_worker_runtime_conf(mut self, conf: WorkerRuntimeOpts) -> Self {
            let _ = self.worker_runtime_conf.insert(conf);
            self
        }

        fn set_jsx_import_source_config(mut self, config: JsxImportSourceConfig) -> Self {
            let _ = self.jsx_import_source_config.insert(config);
            self
        }

        fn add_static_pattern(mut self, pat: &str) -> Self {
            self.static_patterns.push(pat.to_string());
            self
        }

        fn extend_static_patterns<I>(mut self, iter: I) -> Self
        where
            I: IntoIterator<Item = String>,
        {
            self.static_patterns.extend(iter);
            self
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_module_code_no_eszip() {
        let (worker_pool_tx, _) = mpsc::unbounded_channel::<UserWorkerMsgs>();

        DenoRuntime::<()>::new(
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
                maybe_jsx_import_source_config: None,
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
        let bin_eszip = generate_binary_eszip(path_buf, emitter_factory.clone(), None, None, None)
            .await
            .unwrap();
        fs::remove_file("./test_cases/eszip-source-test.ts").unwrap();

        let eszip_code = bin_eszip.into_bytes();

        let runtime = DenoRuntime::<()>::new(
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
                maybe_jsx_import_source_config: None,
            },
            None,
        )
        .await;

        let mut rt = runtime.unwrap();
        let main_mod_ev = rt.js_runtime.mod_evaluate(rt.main_module_id);
        let _ = rt
            .js_runtime
            .run_event_loop(PollEventLoopOptions::default())
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
        let read_is_even = rt.to_value_mut::<serde_json::Value>(&read_is_even_global);
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
        let binary_eszip = generate_binary_eszip(file, emitter_factory.clone(), None, None, None)
            .await
            .unwrap();

        let eszip_code = binary_eszip.into_bytes();

        let runtime = DenoRuntime::<()>::new(
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
                maybe_jsx_import_source_config: None,
            },
            None,
        )
        .await;

        let mut rt = runtime.unwrap();
        let main_mod_ev = rt.js_runtime.mod_evaluate(rt.main_module_id);
        let _ = rt
            .js_runtime
            .run_event_loop(PollEventLoopOptions::default())
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
        let read_is_even = rt.to_value_mut::<serde_json::Value>(&read_is_even_global);
        assert_eq!(read_is_even.unwrap().to_string(), "true");
        std::mem::drop(main_mod_ev);
    }

    // Main Runtime should have access to `EdgeRuntime`
    #[tokio::test]
    #[serial]
    async fn test_main_runtime_creation() {
        let mut runtime = RuntimeBuilder::new().build().await;

        {
            let scope = &mut runtime.js_runtime.handle_scope();
            let context = scope.get_current_context();
            let inner_scope = &mut v8::ContextScope::new(scope, context);
            let global = context.global(inner_scope);
            let edge_runtime_key: v8::Local<v8::Value> =
                serde_v8::to_v8(inner_scope, "EdgeRuntime").unwrap();
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
        let mut runtime = RuntimeBuilder::new()
            .set_worker_runtime_conf(WorkerRuntimeOpts::UserWorker(Default::default()))
            .build()
            .await;

        {
            let scope = &mut runtime.js_runtime.handle_scope();
            let context = scope.get_current_context();
            let inner_scope = &mut v8::ContextScope::new(scope, context);
            let global = context.global(inner_scope);
            let edge_runtime_key: v8::Local<v8::Value> =
                serde_v8::to_v8(inner_scope, "EdgeRuntime").unwrap();
            assert!(global
                .get(inner_scope, edge_runtime_key)
                .unwrap()
                .is_undefined(),);
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_main_rt_fs() {
        let mut main_rt = RuntimeBuilder::new().set_std_env().build().await;

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
            main_rt.to_value_mut::<serde_json::Value>(&global_value_deno_read_file_script);
        assert_eq!(
            fs_read_result.unwrap().as_str().unwrap(),
            "{\n  \"hello\": \"world\"\n}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_jsx_import_source() {
        let mut main_rt = RuntimeBuilder::new()
            .set_std_env()
            .set_path("./test_cases/jsx-preact")
            .set_jsx_import_source_config(JsxImportSourceConfig {
                default_specifier: Some("https://esm.sh/preact".to_string()),
                default_types_specifier: None,
                module: "jsx-runtime".to_string(),
                base_url: Url::from_file_path(std::env::current_dir().unwrap()).unwrap(),
            })
            .build()
            .await;

        let _main_mod_ev = main_rt.js_runtime.mod_evaluate(main_rt.main_module_id);
        let _ = main_rt
            .js_runtime
            .run_event_loop(PollEventLoopOptions::default())
            .await;

        let global_value_deno_read_file_script = main_rt
            .js_runtime
            .execute_script(
                "<anon>",
                ModuleCodeString::from(
                    r#"
                        globalThis.hello;
                    "#
                    .to_string(),
                ),
            )
            .unwrap();

        let jsx_read_result =
            main_rt.to_value_mut::<serde_json::Value>(&global_value_deno_read_file_script);
        assert_eq!(
            jsx_read_result.unwrap().to_string(),
            r#"{"type":"div","props":{"children":"Hello"},"__k":null,"__":null,"__b":0,"__e":null,"__c":null,"__v":-1,"__i":-1,"__u":0}"#
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
        let mut user_rt = RuntimeBuilder::new()
            .set_worker_runtime_conf(WorkerRuntimeOpts::UserWorker(Default::default()))
            .add_static_pattern("./test_cases/**/*.md")
            .build()
            .await;

        let user_rt_execute_scripts = user_rt
            .js_runtime
            .execute_script(
                "<anon>",
                ModuleCodeString::from(
                    // NOTE: Base path is `./test_cases/main`.
                    r#"Deno.readTextFileSync("content.md")"#.to_string(),
                ),
            )
            .unwrap();
        let serde_deno_env = user_rt
            .to_value_mut::<serde_json::Value>(&user_rt_execute_scripts)
            .unwrap();

        assert_eq!(
            serde_deno_env,
            deno_core::serde_json::Value::String(String::from("Some test file"))
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_os_ops() {
        let mut user_rt = RuntimeBuilder::new()
            .set_worker_runtime_conf(WorkerRuntimeOpts::UserWorker(Default::default()))
            .build()
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
            .to_value_mut::<serde_json::Value>(&user_rt_execute_scripts)
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

        let mut main_rt = RuntimeBuilder::new().set_std_env().build().await;
        let mut user_rt = RuntimeBuilder::new()
            .set_worker_runtime_conf(WorkerRuntimeOpts::UserWorker(Default::default()))
            .build()
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
            main_rt.to_value_mut::<serde_json::Value>(&main_deno_env_get_supa_test);
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
            user_rt.to_value_mut::<serde_json::Value>(&user_deno_env_get_supa_test);
        assert!(user_serde_deno_env.unwrap().is_null());
    }

    fn create_basic_user_runtime_builder<T, U>(
        path: &str,
        memory_limit_mb: T,
        worker_timeout_ms: U,
        static_patterns: &[&str],
    ) -> RuntimeBuilder
    where
        T: Into<Option<u64>>,
        U: Into<Option<u64>>,
    {
        let default_opt = UserWorkerRuntimeOpts::default();
        let memory_limit_mb = memory_limit_mb
            .into()
            .unwrap_or(default_opt.memory_limit_mb);
        let worker_timeout_ms = worker_timeout_ms
            .into()
            .unwrap_or(default_opt.worker_timeout_ms);

        RuntimeBuilder::new()
            .set_path(path)
            .set_worker_runtime_conf(WorkerRuntimeOpts::UserWorker(UserWorkerRuntimeOpts {
                memory_limit_mb,
                worker_timeout_ms,
                cpu_time_soft_limit_ms: 100,
                cpu_time_hard_limit_ms: 200,
                force_create: true,
                ..default_opt
            }))
            .extend_static_patterns(static_patterns.iter().map(|it| String::from(*it)))
    }

    #[tokio::test]
    #[serial]
    async fn test_array_buffer_allocation_below_limit() {
        let mut user_rt =
            create_basic_user_runtime_builder("./test_cases/array_buffers", 20, 1000, &[])
                .build()
                .await;

        let (_tx, duplex_stream_rx) = mpsc::unbounded_channel::<DuplexStreamEntry>();
        let (result, _) = user_rt.run(duplex_stream_rx, None, None).await;

        assert!(result.is_ok(), "expected no errors");

        // however, mem checker must be raised because it aggregates heap usage
        assert!(user_rt.mem_check.state.read().unwrap().exceeded);
    }

    #[tokio::test]
    #[serial]
    async fn test_array_buffer_allocation_above_limit() {
        let mut user_rt =
            create_basic_user_runtime_builder("./test_cases/array_buffers", 15, 1000, &[])
                .build()
                .await;

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
        let mut user_rt = create_basic_user_runtime_builder(
            path,
            memory_limit_mb,
            worker_timeout_ms,
            static_patterns,
        )
        .build()
        .await;

        let waker = user_rt.js_runtime.op_state().borrow().waker.clone();
        let handle = user_rt.js_runtime.v8_isolate().thread_safe_handle();

        user_rt.add_memory_limit_callback(move |_| {
            handle.terminate_execution();
            waker.wake();
            callback_tx.send(()).unwrap();
        });

        let wait_fut = async move {
            let (result, _) = user_rt.run(duplex_stream_rx, None, None).await;

            assert!(result
                .unwrap_err()
                .to_string()
                .ends_with("Error: execution terminated"));

            callback_rx.recv().await.unwrap();

            assert!(user_rt.mem_check.state.read().unwrap().exceeded);
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

    #[tokio::test]
    #[serial]
    async fn test_user_worker_permission() {
        struct Ctx;

        impl GetRuntimeContext for Ctx {
            fn get_runtime_context() -> impl Serialize {
                serde_json::json!({
                    "shouldBootstrapMockFnThrowError": true,
                })
            }
        }

        let mut user_rt = create_basic_user_runtime_builder(
            "./test_cases/user-worker-san-check",
            None,
            None,
            &["./test_cases/user-worker-san-check/.blocklisted"],
        )
        .set_context::<Ctx>()
        .build()
        .await;

        let (_tx, duplex_stream_rx) = mpsc::unbounded_channel();

        user_rt.run(duplex_stream_rx, None, None).await.0.unwrap();
    }

    #[tokio::test]
    #[serial]
    #[should_panic]
    async fn test_load_corrupted_eszip_v1() {
        let mut user_rt = RuntimeBuilder::new()
            .set_path("./test_cases/eszip-migration/npm-supabase-js")
            .set_eszip("./test_cases/eszip-migration/npm-supabase-js/v1_corrupted.eszip")
            .await
            .unwrap()
            .set_worker_runtime_conf(WorkerRuntimeOpts::UserWorker(Default::default()))
            .build()
            .await;

        let (_tx, duplex_stream_rx) = mpsc::unbounded_channel();

        user_rt.run(duplex_stream_rx, None, None).await.0.unwrap();
    }
}
