use crate::onnx::ensure_onnx_env_init;
use anyhow::{anyhow, bail, Context};
use base_rt::BlockingScopeCPUUsageMetricExt;
use convert_case::Casing;
use deno_core::error::AnyError;
use deno_core::url::Url;
use deno_core::{serde_json, V8CrossThreadTaskSpawner, V8TaskSpawner};
use deno_core::{JsRuntime, OpState};
use futures::future::{BoxFuture, LocalBoxFuture};
use futures::{FutureExt, StreamExt, TryFutureExt};
use futures_util::io::AllowStdIo;
use once_cell::sync::Lazy;
use ort::{
    CPUExecutionProvider, CUDAExecutionProvider, ExecutionProvider, ExecutionProviderDispatch,
    GraphOptimizationLevel, Session, SessionBuilder,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::env::VarError;
use std::future::Future;
use std::hash::Hasher;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use tokenizers::Tokenizer;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tokio_util::compat::FuturesAsyncWriteCompatExt;
use tracing::{debug, error, info, info_span, instrument, trace, Instrument};
use xxhash_rust::xxh3::Xxh3;

type TaskMap = RwLock<HashMap<Cow<'static, str>, Arc<RwLock<Option<Result<Task, Arc<AnyError>>>>>>>;
type OnnxSessionMap = Mutex<HashMap<Url, Arc<Mutex<Option<Arc<Session>>>>>>;

static TASKS: Lazy<TaskMap> = Lazy::new(TaskMap::default);
static ONNX_SESSIONS: Lazy<OnnxSessionMap> = Lazy::new(OnnxSessionMap::default);

// Can be handled as T | T[] in typescript
#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum PipelineBatchIO<T> {
    Single(T),
    Batch(Vec<T>),
}

#[derive(Debug, Clone)]
struct Task {
    onnx_session: Arc<Session>,
    maybe_tokenizer: Option<Arc<Tokenizer>>,
    maybe_config: Option<Arc<Map<String, Value>>>,
}

impl Task {
    #[instrument(skip_all, fields(pipeline = &*def.name()))]
    async fn new<T>(def: &T) -> Result<Self, AnyError>
    where
        T: PipelineDefinition,
    {
        let onnx_session = async move {
            let model_url = def.model_url(None)?;
            let store = {
                let mut guard = ONNX_SESSIONS.lock().await;

                guard
                    .entry(model_url.clone())
                    .or_insert_with(Arc::default)
                    .clone()
            };

            let mut guard = store.lock().await;

            if let Some(already_initialized_onnx_session) = guard.clone() {
                return Ok::<_, AnyError>(already_initialized_onnx_session);
            }

            let onnx_session = fetch_and_cache_from_url(&model_url, "models")
                .await
                .and_then(|it| {
                    if let Some(err) = ensure_onnx_env_init() {
                        return Err(anyhow!("failed to create onnx environment: {err}"));
                    }

                    def.session_builder()?
                        .with_execution_providers(def.execution_providers())
                        .context("failed to register execution providers")?
                        .commit_from_file(it)
                        .context("failed to commit model to session")
                })
                .map(Arc::new)?;

            *guard = Some(onnx_session.clone());

            Ok(onnx_session)
        }
        .instrument(info_span!("session"));

        let maybe_tokenizer = async move {
            if let Some(url) = def.tokenizer_url(None) {
                let buf = tokio::fs::read(fetch_and_cache_from_url(&url?, "tokenizers").await?)
                    .map_err(AnyError::new)
                    .await?;

                let mut tokenizer = Tokenizer::from_bytes(buf).map_err(|e| anyhow!(e))?;

                def.configure_tokenizer(&mut tokenizer);

                Ok(Some(Arc::new(tokenizer)))
            } else {
                Ok::<_, AnyError>(None)
            }
        }
        .instrument(info_span!("tokenizer"));

        let maybe_config = async move {
            if let Some(url) = def.config_url(None) {
                let buf = tokio::fs::read(fetch_and_cache_from_url(&url?, "configs").await?)
                    .map_err(AnyError::new)
                    .await?;

                let config = serde_json::from_slice(&buf).map_err(|e| anyhow!(e))?;

                Ok(Some(Arc::new(config)))
            } else {
                Ok::<_, AnyError>(None)
            }
        }
        .instrument(info_span!("config"));

        tokio::try_join!(onnx_session, maybe_tokenizer, maybe_config).map(
            |(onnx_session, maybe_tokenizer, maybe_config)| Self {
                onnx_session,
                maybe_tokenizer,
                maybe_config,
            },
        )
    }
}

#[instrument(skip(url), fields(url = %url))]
async fn fetch_and_cache_from_url(url: &Url, kind: &'static str) -> Result<PathBuf, AnyError> {
    let cache_id = fxhash::hash(url.as_str()).to_string();
    let download_dir = ort::sys::internal::dirs::cache_dir()
        .context("could not determine cache directory")?
        .join(kind)
        .join(cache_id);

    std::fs::create_dir_all(&download_dir).context("could not able to create directories")?;

    let filename = url
        .path_segments()
        .and_then(Iterator::last)
        .context("missing filename in URL")?;

    let filepath = download_dir.join(filename);
    let checksum_path = filepath.with_file_name(format!("{filename}.checksum"));

    let is_filepath_exists = tokio::fs::try_exists(&filepath)
        .await
        .map_err(AnyError::from)?;

    let is_checksum_path_exists = tokio::fs::try_exists(&checksum_path)
        .await
        .map_err(AnyError::from)?;

    let is_checksum_valid = is_checksum_path_exists
        && 'scope: {
            let Some(old_checksum_str) = tokio::fs::read_to_string(&checksum_path).await.ok()
            else {
                break 'scope false;
            };

            let filepath = filepath.clone();
            let checksum = tokio::task::spawn_blocking(move || {
                let mut file = std::fs::File::open(filepath).ok()?;
                let mut hasher = Xxh3::new();
                let _ = std::io::copy(&mut file, &mut hasher).ok()?;

                Some(hasher.finish().to_be_bytes())
            })
            .await;

            let Ok(Some(checksum)) = checksum else {
                break 'scope false;
            };

            old_checksum_str == faster_hex::hex_string(&checksum)
        };

    let span = info_span!("download", filepath = %filepath.to_string_lossy());
    async move {
        if is_filepath_exists && is_checksum_valid {
            info!("binary already exists, skipping download");
            Ok(filepath.clone())
        } else {
            info!("downloading binary");

            if is_filepath_exists {
                let _ = tokio::fs::remove_file(&filepath).await;
            }

            use reqwest::*;

            let resp = Client::builder()
                .build()
                .context("failed to create http client")?
                .get(url.clone())
                .send()
                .await
                .context("failed to download")?;

            let len = resp
                .headers()
                .get(header::CONTENT_LENGTH)
                .map(|it| it.to_str().map_err(AnyError::new))
                .transpose()?
                .map(|it| it.parse::<usize>().map_err(AnyError::new))
                .transpose()?
                .context("invalid Content-Length header")?;

            debug!(total_bytes = len);

            let file = tokio::fs::File::create(&filepath)
                .await
                .context("failed to create file")?;

            let mut stream = resp.bytes_stream();
            let mut writer = tokio::io::BufWriter::new(file);
            let mut hasher = AllowStdIo::new(Xxh3::new()).compat_write();
            let mut written = 0;

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.context("failed to get chunks")?;

                written += tokio::io::copy(&mut chunk.as_ref(), &mut writer)
                    .await
                    .context("failed to store chunks to file")?;

                let _ = tokio::io::copy(&mut chunk.as_ref(), &mut hasher)
                    .await
                    .context("failed to calculate checksum")?;

                trace!(bytes_written = written);
            }

            let checksum_str = {
                let hasher = hasher.into_inner().into_inner();
                faster_hex::hex_string(&hasher.finish().to_be_bytes())
            };

            if written == len as u64 {
                info!({ bytes_written = written, checksum = &checksum_str }, "done");

                let mut checksum_file = tokio::fs::File::create(&checksum_path)
                    .await
                    .context("failed to create checksum file")?;

                let _ = checksum_file
                    .write(checksum_str.as_bytes())
                    .await
                    .context("failed to write checksum to file system")?;

                Ok(filepath)
            } else {
                error!({ expected = len, got = written }, "bytes mismatch");
                bail!("error copying data to file: expected {len} length, but got {written}");
            }
        }
    }
    .instrument(span)
    .await
}

struct PipelineRequest<T, TO, U> {
    input: T,
    options: Option<TO>,
    tx: oneshot::Sender<Result<U, AnyError>>,
}

struct PipelineTx<T: PipelineDefinition>(
    #[allow(unused)] mpsc::UnboundedSender<PipelineRequest<T::Input, T::InputOptions, T::Output>>,
);

struct PipelineWeakTx<T: PipelineDefinition>(
    mpsc::WeakUnboundedSender<PipelineRequest<T::Input, T::InputOptions, T::Output>>,
);

pub(crate) trait PipelineDefinition: Send + Sync {
    type Input: DeserializeOwned + Send + Sync;
    type InputOptions: DeserializeOwned + Send + Sync;
    type Output: Serialize + Send;

    fn make() -> Self;

    fn name(&self) -> Cow<'static, str>;

    fn model_url(&self, requested_variation: Option<&str>) -> Result<Url, AnyError>;

    fn tokenizer_url(&self, _requested_variation: Option<&str>) -> Option<Result<Url, AnyError>> {
        None
    }

    fn config_url(&self, _requested_variation: Option<&str>) -> Option<Result<Url, AnyError>> {
        None
    }

    fn configure_tokenizer(&self, tokenizer: &mut Tokenizer) {
        let Some(truncation) = tokenizer.get_truncation_mut() else {
            error!("failed to configure the max length of truncation parameters");
            return;
        };

        // model's default max length is 128. Increase it to 512.
        truncation.max_length = 512;
    }

    fn session_builder(&self) -> Result<SessionBuilder, AnyError> {
        let orm_threads = std::env::var("OMP_NUM_THREADS")
            .map_or(None, |val| val.parse::<usize>().ok())
            .unwrap_or(1);

        Ok(Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            // NOTE(Nyannyacha): This is set to prevent memory leaks caused by different input
            // shapes.
            //
            // Backgrounds:
            // [1]: https://github.com/microsoft/onnxruntime/issues/11118
            // [2]: https://github.com/microsoft/onnxruntime/blob/main/onnxruntime/core/framework/session_options.h#L95-L110
            .with_memory_pattern(false)?
            .with_intra_threads(orm_threads)?)
    }

    fn execution_providers(&self) -> Box<dyn Iterator<Item = ExecutionProviderDispatch>> {
        Box::new(
            [
                // NOTE(Nyannacha): See the comment above. This makes `enable_cpu_mem_arena` set to
                // False.
                //
                // Backgrounds:
                // [1]: https://docs.rs/ort/2.0.0-rc.4/src/ort/execution_providers/cpu.rs.html#9-18
                // [2]: https://docs.rs/ort/2.0.0-rc.4/src/ort/execution_providers/cpu.rs.html#46-50
                CPUExecutionProvider::default().build(),
            ]
            .into_iter(),
        )
    }

    fn run(
        &self,
        session: &Session,
        tokenizer: Option<&Tokenizer>,
        config: Option<&Map<String, Value>>,
        input: &Self::Input,
        options: Option<&Self::InputOptions>,
    ) -> Result<Self::Output, AnyError>;
}

#[derive(Debug, Clone)]
pub(crate) struct WithCUDAExecutionProvider<T: PipelineDefinition> {
    inner: T,
}

impl<T: PipelineDefinition> PipelineDefinition for WithCUDAExecutionProvider<T> {
    type Input = T::Input;
    type InputOptions = T::InputOptions;
    type Output = T::Output;

    fn make() -> Self {
        WithCUDAExecutionProvider { inner: T::make() }
    }

    fn name(&self) -> Cow<'static, str> {
        self.inner.name()
    }

    fn model_url(&self, requested_variation: Option<&str>) -> Result<Url, AnyError> {
        self.inner.model_url(requested_variation)
    }

    fn tokenizer_url(&self, requested_variation: Option<&str>) -> Option<Result<Url, AnyError>> {
        self.inner.tokenizer_url(requested_variation)
    }

    fn config_url(&self, requested_variation: Option<&str>) -> Option<Result<Url, AnyError>> {
        self.inner.config_url(requested_variation)
    }

    fn configure_tokenizer(&self, tokenizer: &mut Tokenizer) {
        self.inner.configure_tokenizer(tokenizer)
    }

    fn execution_providers(&self) -> Box<dyn Iterator<Item = ExecutionProviderDispatch>> {
        let cuda = CUDAExecutionProvider::default();
        let providers = match cuda.is_available() {
            Ok(is_cuda_available) => {
                debug!(cuda_support = is_cuda_available);
                if is_cuda_available {
                    vec![cuda.build()]
                } else {
                    vec![]
                }
            }

            _ => vec![],
        };

        Box::new(
            providers
                .into_iter()
                .chain(self.inner.execution_providers()),
        )
    }

    fn run(
        &self,
        session: &Session,
        tokenizer: Option<&Tokenizer>,
        config: Option<&Map<String, Value>>,
        input: &Self::Input,
        options: Option<&Self::InputOptions>,
    ) -> Result<Self::Output, AnyError> {
        self.inner.run(session, tokenizer, config, input, options)
    }
}

fn try_create_pipeline_fut<T>(
    state: &mut OpState,
    def: Arc<T>,
    task: Task,
) -> impl Future<Output = ()>
where
    T: PipelineDefinition + 'static,
{
    let cross_thread_spawner = state.borrow::<V8CrossThreadTaskSpawner>().clone();
    let mut req_rx = {
        let (req_tx, req_rx) =
            mpsc::unbounded_channel::<PipelineRequest<T::Input, T::InputOptions, T::Output>>();

        let _ = state.try_take::<PipelineTx<T>>();
        let _ = state.try_take::<PipelineWeakTx<T>>();

        state.put(PipelineWeakTx::<T>(req_tx.downgrade()));
        state.put(PipelineTx::<T>(req_tx));

        req_rx
    };

    async move {
        loop {
            let Some(req) = req_rx.recv().await else {
                break;
            };

            let def = def.clone();
            let session = task.onnx_session.clone();
            let tokenizer = task.maybe_tokenizer.clone();
            let config = task.maybe_config.clone();

            cross_thread_spawner.spawn(move |state| {
                drop(
                    JsRuntime::op_state_from(state)
                        .borrow_mut()
                        .spawn_cpu_accumul_blocking_scope(move || {
                            let result = def.run(
                                session.as_ref(),
                                tokenizer.as_deref(),
                                config.as_deref(),
                                &req.input,
                                req.options.as_ref(),
                            );

                            if let Err(_result) = req.tx.send(result) {
                                error!("failed to send inference result");
                            };
                        }),
                )
            })
        }
    }
}

fn ensure_task_ready<T>(def: &T) -> BoxFuture<'_, Result<Task, Arc<AnyError>>>
where
    T: PipelineDefinition,
{
    async move {
        let name = def.name();
        let guard = TASKS.read().await;
        if let Some(task) = guard.get(&name).cloned() {
            let guard = {
                drop(guard);
                task.read().await
            };

            if let Some(inner) = &*guard {
                return inner.clone();
            } else {
                let mut guard = {
                    drop(guard);
                    task.write().await
                };

                if let Some(inner) = &*guard {
                    return inner.clone();
                }

                let task = Task::new(def).await.map_err(Arc::new);
                let _ = guard.insert(task.clone());

                return task;
            }
        }

        let mut guard = {
            drop(guard);
            TASKS.write().await
        };

        if guard.contains_key(&name) {
            drop(guard);
            return ensure_task_ready(def).await;
        }

        let maybe_old = guard.insert(name, Arc::default());
        debug_assert!(maybe_old.is_none());

        drop(guard);
        ensure_task_ready(def).await
    }
    .boxed()
}

pub fn compose_url_env_key(
    prefix: &'static str,
    def_name: &str,
    variation: Option<&str>,
) -> String {
    let variation = variation.map(|it| format!("_{it}")).unwrap_or_default();

    format!(
        "{}_{prefix}_{def_name}{variation}",
        env!("CARGO_CRATE_NAME")
    )
    .to_case(convert_case::Case::UpperSnake)
}

fn try_get_url_from_env<T>(
    prefix: &'static str,
    def: &T,
    variation: Option<&str>,
) -> Option<Result<Url, AnyError>>
where
    T: PipelineDefinition,
{
    let key = compose_url_env_key(prefix, &def.name(), variation);

    let value = match std::env::var(key) {
        Ok(value) => value,
        Err(err) => match err {
            VarError::NotPresent => return None,
            err @ VarError::NotUnicode(_) => return Some(Err(AnyError::from(err))),
        },
    };

    Some(Url::parse(&value).context("failed to parse URL"))
}

pub fn try_get_model_url_from_env<T>(
    def: &T,
    variation: Option<&str>,
) -> Option<Result<Url, AnyError>>
where
    T: PipelineDefinition,
{
    try_get_url_from_env("model_url", def, variation)
}

pub fn try_get_tokenizer_url_from_env<T>(
    def: &T,
    variation: Option<&str>,
) -> Option<Result<Url, AnyError>>
where
    T: PipelineDefinition,
{
    try_get_url_from_env("tokenizer_url", def, variation)
}

pub fn try_get_config_url_from_env<T>(
    def: &T,
    variation: Option<&str>,
) -> Option<Result<Url, AnyError>>
where
    T: PipelineDefinition,
{
    try_get_url_from_env("config_url", def, variation)
}

trait PipelineRunner {
    fn run(
        &self,
        state: Rc<RefCell<OpState>>,
        input: serde_json::Value,
        options: serde_json::Value,
    ) -> LocalBoxFuture<'_, Result<serde_json::Value, AnyError>>;
}

impl<T: PipelineDefinition + 'static> PipelineRunner for T {
    fn run(
        &self,
        state: Rc<RefCell<OpState>>,
        input: serde_json::Value,
        options: serde_json::Value,
    ) -> LocalBoxFuture<'_, Result<serde_json::Value, AnyError>> {
        async move {
            let req_tx = state
                .borrow()
                .try_borrow::<PipelineWeakTx<T>>()
                .and_then(|it| it.0.upgrade())
                .context("failed to upgrade request sender")?;

            let deserialized_input =
                serde_json::from_value::<T::Input>(input).context("cannot deserialize input")?;

            let deserialized_options = serde_json::from_value::<Option<T::InputOptions>>(options)
                .context("cannot deserialize input")?;

            let (result_tx, result_rx) = oneshot::channel::<Result<T::Output, AnyError>>();

            req_tx.send(PipelineRequest {
                input: deserialized_input,
                options: deserialized_options,
                tx: result_tx,
            })?;

            let result = result_rx
                .await
                .context("failed to get inference result from receiver")??;

            serde_json::to_value(result).context("failed to serialize inference result")
        }
        .boxed_local()
    }
}

#[derive(Default)]
struct PipelineRunners(HashMap<Cow<'static, str>, Arc<dyn PipelineRunner>>);

pub enum PipelineInitArg<T> {
    Simple(T),
    WithVariation { def: T, variation: String },
}

impl<T: PipelineDefinition> PipelineDefinition for PipelineInitArg<T> {
    type Input = T::Input;
    type InputOptions = T::InputOptions;
    type Output = T::Output;

    fn make() -> Self {
        unreachable!();
    }

    fn name(&self) -> Cow<'static, str> {
        if let Some(variation) = self.variation() {
            format!("{}:{variation}", &self.def().name()).into()
        } else {
            self.def().name()
        }
    }

    fn model_url(&self, requested_variation: Option<&str>) -> Result<Url, anyhow::Error> {
        debug_assert!(requested_variation.is_none());
        self.def().model_url(self.variation())
    }

    fn tokenizer_url(&self, requested_variation: Option<&str>) -> Option<Result<Url, AnyError>> {
        debug_assert!(requested_variation.is_none());
        self.def().tokenizer_url(self.variation())
    }

    fn config_url(&self, requested_variation: Option<&str>) -> Option<Result<Url, AnyError>> {
        debug_assert!(requested_variation.is_none());
        self.def().config_url(requested_variation)
    }

    fn configure_tokenizer(&self, tokenizer: &mut Tokenizer) {
        self.def().configure_tokenizer(tokenizer)
    }

    fn run(
        &self,
        session: &Session,
        tokenizer: Option<&Tokenizer>,
        config: Option<&Map<String, Value>>,
        input: &Self::Input,
        options: Option<&Self::InputOptions>,
    ) -> Result<Self::Output, AnyError> {
        self.def().run(session, tokenizer, config, input, options)
    }
}

impl<T> PipelineInitArg<T> {
    fn def(&self) -> &T {
        match self {
            Self::Simple(def) => def,
            Self::WithVariation { def, .. } => def,
        }
    }

    fn variation(&self) -> Option<&str> {
        match self {
            Self::Simple(_) => None,
            Self::WithVariation { variation, .. } => Some(variation.as_str()),
        }
    }
}

pub async fn init<T>(
    state: Option<Rc<RefCell<OpState>>>,
    arg: PipelineInitArg<T>,
) -> Result<(), AnyError>
where
    T: PipelineDefinition + Clone + 'static,
{
    let def = Arc::new(arg);
    let name = def.name();
    let spawner = 'scope: {
        let Some(state) = state else {
            break 'scope None;
        };

        let spawner = {
            let mut state = state.borrow_mut();
            let spawner = state.borrow::<V8TaskSpawner>().clone();
            let runners = if state.has::<PipelineRunners>() {
                state.borrow_mut::<PipelineRunners>()
            } else {
                state.put(PipelineRunners::default());
                state.borrow_mut::<PipelineRunners>()
            };

            if runners.0.contains_key(&name) {
                return Ok(());
            }

            runners.0.insert(name, def.clone());
            spawner
        };

        Some(spawner)
    };

    let task = match ensure_task_ready(&*def).await {
        Ok(task) => task,
        Err(err) => bail!("failed to create task: {err:?}"),
    };

    if let Some(spawner) = spawner {
        spawner.spawn(move |scope| {
            let state = JsRuntime::op_state_from(scope);
            tokio::task::spawn(try_create_pipeline_fut(&mut state.borrow_mut(), def, task));
        });
    }

    Ok(())
}

pub enum PipelineRunArg<'l> {
    Simple(&'l str),
    WithVariation { name: &'l str, variation: &'l str },
}

impl<'l> PipelineRunArg<'l> {
    fn name(&self) -> &'l str {
        match self {
            Self::Simple(name) => name,
            Self::WithVariation { name, .. } => name,
        }
    }

    fn variation(&self) -> Option<&'l str> {
        match self {
            Self::Simple(_) => None,
            Self::WithVariation { variation, .. } => Some(variation),
        }
    }

    fn runner_key(&self) -> Cow<'l, str> {
        match self {
            Self::Simple(name) => (*name).into(),
            Self::WithVariation { name, variation } => format!("{name}:{variation}").into(),
        }
    }
}

pub async fn run(
    state: Rc<RefCell<OpState>>,
    input: serde_json::Value,
    options: serde_json::Value,
    arg: PipelineRunArg<'_>,
) -> Result<serde_json::Value, AnyError> {
    let name = arg.name();
    let variation = arg.variation().unwrap_or("null");
    let key = arg.runner_key();
    let runner = {
        let mut state = state.borrow_mut();
        let runners = if state.has::<PipelineRunners>() {
            state.borrow::<PipelineRunners>()
        } else {
            state.put(PipelineRunners::default());
            state.borrow::<PipelineRunners>()
        };

        runners.0.get(&key).cloned()
    }
    .with_context(|| {
        format!(
            "pipeline is unsupported or not initialized: (name: {name}, variation: {variation})"
        )
    })?;

    runner.run(state, input, options).await
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
struct CleanupResult {
    dropped_task_count: usize,
    dropped_session_count: usize,
}

pub async fn cleanup() -> Result<serde_json::Value, AnyError> {
    let mut result = CleanupResult::default();

    {
        let mut guard = TASKS.write().await;
        let mut to_be_removed = vec![];
        for (key, task) in &mut *guard {
            if Arc::strong_count(task) > 1 {
                continue;
            }

            to_be_removed.push(key.clone());
        }
        for key in to_be_removed {
            let old_task = guard.remove(&key);
            debug_assert!(old_task.is_some());
            result.dropped_task_count += 1;
        }
    }

    {
        let mut guard = ONNX_SESSIONS.lock().await;
        let mut to_be_removed = vec![];
        for (key, store) in &mut *guard {
            let maybe_session = store.lock().await;
            if let Some(session) = &*maybe_session {
                if Arc::strong_count(session) > 1 {
                    continue;
                }
            }
            if Arc::strong_count(store) > 1 {
                continue;
            }

            to_be_removed.push(key.clone());
        }
        for key in to_be_removed {
            let old_store = guard.remove(&key);
            debug_assert!(old_store.is_some());
            result.dropped_session_count += 1;
        }
    }

    serde_json::to_value(result).context("failed to serialize cleanup result")
}
