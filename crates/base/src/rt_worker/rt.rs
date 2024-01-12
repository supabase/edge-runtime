use once_cell::sync::Lazy;

pub static SUPERVISOR_RT: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("sb-supervisor")
        .build()
        .unwrap()
});

pub static WORKER_RT: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("sb-worker")
        .build()
        .unwrap()
});
