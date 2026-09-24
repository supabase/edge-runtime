use std::sync::Arc;

use anyhow::bail;
use deno::deno_tls::deno_native_certs::load_native_certs;
use deno::deno_tls::rustls::RootCertStore;
use deno::deno_tls::webpki_roots;
use deno::deno_tls::RootCertStoreProvider;
use deno_core::error::AnyError;
use ext_runtime::cert::ValueRootCertStoreProvider;

fn add_mozilla_roots(root_cert_store: &mut RootCertStore) {
  root_cert_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
}

pub fn get_root_cert_store_provider(
) -> Result<Arc<dyn RootCertStoreProvider>, AnyError> {
  // Create and populate a root cert store based on environment variable.
  // Reference: https://github.com/denoland/deno/blob/v1.37.0/cli/args/mod.rs#L467
  let mut root_cert_store = RootCertStore::empty();
  let ca_stores: Vec<String> = (|| {
    let env_ca_store = std::env::var("DENO_TLS_CA_STORE").ok()?;
    Some(
      env_ca_store
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect(),
    )
  })()
  .unwrap_or_else(|| vec!["mozilla".to_string()]);

  for store in ca_stores.iter() {
    match store.as_str() {
      "mozilla" => add_mozilla_roots(&mut root_cert_store),
      "system" => {
        let roots = load_native_certs().expect("could not load platform certs");
        for root in roots {
          root_cert_store
            .add((&*root.0).into())
            .expect("Failed to add platform cert to root cert store");
        }
      }
      _ => {
        bail!(
          concat!(
            "Unknown certificate store \"{0}\" specified ",
            "(allowed: \"system,mozilla\")"
          ),
          store
        );
      }
    }
  }

  Ok(Arc::new(ValueRootCertStoreProvider::new(
    root_cert_store.clone(),
  )))
}

#[cfg(test)]
mod tests {
  use deno::deno_tls::rustls::pki_types::Der;
  use deno::deno_tls::rustls::pki_types::TrustAnchor;

  use super::*;

  fn add_test_root(
    root_cert_store: &mut RootCertStore,
    subject: &'static [u8],
  ) {
    root_cert_store.roots.push(TrustAnchor {
      subject: Der::from_slice(subject),
      subject_public_key_info: Der::from_slice(b"test-key"),
      name_constraints: None,
    });
  }

  fn has_root_with_subject(
    root_cert_store: &RootCertStore,
    subject: &[u8],
  ) -> bool {
    root_cert_store
      .roots
      .iter()
      .any(|root| root.subject.as_ref() == subject)
  }

  #[test]
  fn add_mozilla_roots_appends_to_existing_store() {
    let mut root_cert_store = RootCertStore::empty();
    add_test_root(&mut root_cert_store, b"system");
    let root_count_before = root_cert_store.roots.len();

    add_mozilla_roots(&mut root_cert_store);

    assert!(root_cert_store.roots.len() > root_count_before);
    assert!(has_root_with_subject(&root_cert_store, b"system"));
  }
}
