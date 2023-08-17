use deno_core::error::AnyError;
use deno_tls::rustls::RootCertStore;
use deno_tls::RootCertStoreProvider;

pub struct ValueRootCertStoreProvider {
    pub root_cert_store: RootCertStore,
}

impl ValueRootCertStoreProvider {
    pub fn new(root_cert_store: RootCertStore) -> Self {
        Self { root_cert_store }
    }
}

impl RootCertStoreProvider for ValueRootCertStoreProvider {
    fn get_or_try_init(&self) -> Result<&RootCertStore, AnyError> {
        Ok(&self.root_cert_store)
    }
}
