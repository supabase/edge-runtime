use deno_core::error::AnyError;
use deno_tls::deno_native_certs::load_native_certs;
use deno_tls::rustls::RootCertStore;
use deno_tls::{rustls, rustls_pemfile, webpki_roots, RootCertStoreProvider};
use std::io::{BufReader, Cursor};
use std::path::PathBuf;
use thiserror::Error;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaData {
    /// The string is a file path
    File(String),
    /// This variant is not exposed as an option in the CLI, it is used internally
    /// for standalone binaries.
    Bytes(Vec<u8>),
}

#[derive(Error, Debug, Clone)]
pub enum RootCertStoreLoadError {
    #[error("Unknown certificate store \"{0}\" specified (allowed: \"system,mozilla\")")]
    UnknownStore(String),
    #[error("Unable to add pem file to certificate store: {0}")]
    FailedAddPemFile(String),
    #[error("Failed opening CA file: {0}")]
    CaFileOpenError(String),
}

pub fn get_root_cert_store(
    maybe_root_path: Option<PathBuf>,
    maybe_ca_stores: Option<Vec<String>>,
    maybe_ca_data: Option<CaData>,
) -> Result<RootCertStore, RootCertStoreLoadError> {
    let mut root_cert_store = RootCertStore::empty();
    let ca_stores: Vec<String> = maybe_ca_stores
        .or_else(|| {
            let env_ca_store = std::env::var("DENO_TLS_CA_STORE").ok()?;
            Some(
                env_ca_store
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
            )
        })
        .unwrap_or_else(|| vec!["mozilla".to_string()]);

    for store in ca_stores.iter() {
        match store.as_str() {
            "mozilla" => {
                root_cert_store.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.iter().map(
                    |ta| {
                        rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
                            ta.subject,
                            ta.spki,
                            ta.name_constraints,
                        )
                    },
                ));
            }
            "system" => {
                let roots = load_native_certs().expect("could not load platform certs");
                for root in roots {
                    root_cert_store
                        .add(&rustls::Certificate(root.0))
                        .expect("Failed to add platform cert to root cert store");
                }
            }
            _ => {
                return Err(RootCertStoreLoadError::UnknownStore(store.clone()));
            }
        }
    }

    let ca_data = maybe_ca_data.or_else(|| std::env::var("DENO_CERT").ok().map(CaData::File));
    if let Some(ca_data) = ca_data {
        let result = match ca_data {
            CaData::File(ca_file) => {
                let ca_file = if let Some(root) = &maybe_root_path {
                    root.join(&ca_file)
                } else {
                    PathBuf::from(ca_file)
                };
                let certfile = std::fs::File::open(ca_file)
                    .map_err(|err| RootCertStoreLoadError::CaFileOpenError(err.to_string()))?;
                let mut reader = BufReader::new(certfile);
                rustls_pemfile::certs(&mut reader)
            }
            CaData::Bytes(data) => {
                let mut reader = BufReader::new(Cursor::new(data));
                rustls_pemfile::certs(&mut reader)
            }
        };

        match result {
            Ok(certs) => {
                root_cert_store.add_parsable_certificates(&certs);
            }
            Err(e) => {
                return Err(RootCertStoreLoadError::FailedAddPemFile(e.to_string()));
            }
        }
    }

    Ok(root_cert_store)
}
