use once_cell::sync::Lazy;

pub(crate) static SYNC_IO_RT: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("sb-virtualfs-io")
        .build()
        .unwrap()
});
