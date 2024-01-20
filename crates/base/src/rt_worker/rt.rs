use once_cell::sync::Lazy;

pub static SUPERVISOR_RT: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("sb-supervisor")
        .build()
        .unwrap()
});

// NOTE: This pool is for the main and event workers. The reason why they should
// separate from the user worker pool is they can starve them if user workers
// are saturated.
pub static PRIMARY_WORKER_RT: Lazy<tokio_util::task::LocalPoolHandle> =
    Lazy::new(|| tokio_util::task::LocalPoolHandle::new(2));

pub static USER_WORKER_RT: Lazy<tokio_util::task::LocalPoolHandle> = Lazy::new(|| {
    let maybe_pool_size = std::env::var("EDGE_RUNTIME_WORKER_POOL_SIZE")
        .ok()
        .and_then(|it| it.parse::<usize>().ok());

    tokio_util::task::LocalPoolHandle::new(if cfg!(debug_assertions) {
        maybe_pool_size.unwrap_or(1)
    } else {
        maybe_pool_size.unwrap_or(std::thread::available_parallelism().unwrap().get())
    })
});
