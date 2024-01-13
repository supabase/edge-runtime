use sb_core::conn_sync::ConnSync;
use scopeguard::ScopeGuard;
use tokio::sync::watch;

pub fn create_conn_watch() -> (
    ScopeGuard<watch::Sender<ConnSync>, impl FnOnce(watch::Sender<ConnSync>)>,
    watch::Receiver<ConnSync>,
) {
    let (conn_watch_tx, conn_watch_rx) = watch::channel(ConnSync::Want);
    let conn_watch_tx = scopeguard::guard(conn_watch_tx, |tx| tx.send(ConnSync::Recv).unwrap());

    (conn_watch_tx, conn_watch_rx)
}
