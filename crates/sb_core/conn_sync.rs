use deno_core::Resource;
use tokio::sync::watch;

#[derive(Debug, PartialEq, Eq)]
pub enum ConnSync {
    Want,
    Recv,
}

pub struct ConnWatcher(pub Option<watch::Receiver<ConnSync>>);

impl Resource for ConnWatcher {
    fn name(&self) -> std::borrow::Cow<str> {
        "ConnWatcher".into()
    }
}

impl ConnWatcher {
    pub fn get(&self) -> Option<watch::Receiver<ConnSync>> {
        self.0.clone()
    }
}
