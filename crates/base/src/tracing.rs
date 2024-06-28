use ctor::ctor;
use tracing_subscriber::{filter::LevelFilter, EnvFilter};

#[ctor]
fn init_tracing_subscriber() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::OFF.into())
                .from_env_lossy(),
        )
        .with_thread_names(true)
        .init()
}
