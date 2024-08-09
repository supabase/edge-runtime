use deno_core::Resource;
use tokio_util::sync::CancellationToken;

pub struct ConnWatcher(pub Option<CancellationToken>);

impl Resource for ConnWatcher {
    fn name(&self) -> std::borrow::Cow<str> {
        "connWatcher".into()
    }
}

impl ConnWatcher {
    pub fn get(&self) -> Option<CancellationToken> {
        self.0.clone()
    }
}
