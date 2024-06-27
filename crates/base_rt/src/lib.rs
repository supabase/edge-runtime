use std::num::NonZeroUsize;

use once_cell::sync::Lazy;

pub const DEFAULT_PRIMARY_WORKER_POOL_SIZE: usize = 2;
pub const DEFAULT_USER_WORKER_POOL_SIZE: usize = 1;

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
pub static PRIMARY_WORKER_RT: Lazy<tokio_util::task::LocalPoolHandle> = Lazy::new(|| {
    let maybe_pool_size = std::env::var("EDGE_RUNTIME_PRIMARY_WORKER_POOL_SIZE")
        .ok()
        .and_then(|it| it.parse::<usize>().ok())
        .map(|it| {
            if it < DEFAULT_PRIMARY_WORKER_POOL_SIZE {
                DEFAULT_PRIMARY_WORKER_POOL_SIZE
            } else {
                it
            }
        });

    tokio_util::task::LocalPoolHandle::new(
        maybe_pool_size.unwrap_or(DEFAULT_PRIMARY_WORKER_POOL_SIZE),
    )
});

pub static USER_WORKER_RT: Lazy<tokio_util::task::LocalPoolHandle> = Lazy::new(|| {
    let maybe_pool_size = std::env::var("EDGE_RUNTIME_WORKER_POOL_SIZE")
        .ok()
        .and_then(|it| it.parse::<usize>().ok())
        .map(|it| {
            if it < DEFAULT_USER_WORKER_POOL_SIZE {
                DEFAULT_USER_WORKER_POOL_SIZE
            } else {
                it
            }
        });

    tokio_util::task::LocalPoolHandle::new(if cfg!(debug_assertions) {
        maybe_pool_size.unwrap_or(DEFAULT_USER_WORKER_POOL_SIZE)
    } else {
        maybe_pool_size.unwrap_or(
            std::thread::available_parallelism()
                .ok()
                .map(NonZeroUsize::get)
                .unwrap_or(DEFAULT_USER_WORKER_POOL_SIZE),
        )
    })
});
