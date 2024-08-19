use std::{borrow::Cow, cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

use anyhow::bail;
use deno_core::{error::AnyError, OpState};
use futures::{future::LocalBoxFuture, FutureExt};
use once_cell::sync::Lazy;

use crate::pipeline::{self, PipelineDefinition};

mod feature_extraction;
mod text_classification;

type InitializerReturnType = LocalBoxFuture<'static, Result<(), AnyError>>;
type Initializer = Arc<
    dyn (Fn(Option<Rc<RefCell<OpState>>>, Option<String>) -> InitializerReturnType) + Send + Sync,
>;

static DEFS: Lazy<HashMap<Cow<'static, str>, Initializer>> = Lazy::new(|| {
    HashMap::from([
        factory::<pipeline::WithCUDAExecutionProvider<feature_extraction::SupabaseGte>>(),
        factory::<pipeline::WithCUDAExecutionProvider<feature_extraction::GteSmall>>(),
        factory::<
            pipeline::WithCUDAExecutionProvider<text_classification::SentimentAnalysisPipeline>,
        >(),
        factory::<
            pipeline::WithCUDAExecutionProvider<
                text_classification::ZeroShotClassificationPipeline,
            >,
        >(),
    ])
});

fn factory<T>() -> (Cow<'static, str>, Initializer)
where
    T: PipelineDefinition + Clone + 'static,
{
    let def = T::make();
    (def.name(), initializer_factory(def))
}

fn initializer_factory<T>(def: T) -> Initializer
where
    T: PipelineDefinition + Clone + 'static,
{
    Arc::new(move |state, variation| {
        let def = def.clone();
        let arg = if let Some(variation) = variation {
            pipeline::PipelineInitArg::WithVariation { def, variation }
        } else {
            pipeline::PipelineInitArg::Simple(def)
        };

        pipeline::init(state, arg).boxed_local()
    })
}

pub async fn init(
    state: Option<Rc<RefCell<OpState>>>,
    name: &str,
    variation: Option<String>,
) -> Result<(), AnyError> {
    if let Some(initalizer) = DEFS.get(name) {
        (*initalizer)(state, variation).await
    } else {
        bail!("unsupported pipeline: {name}");
    }
}
