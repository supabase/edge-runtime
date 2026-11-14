// Copyright 2018-2024 the Deno authors. All rights reserved. MIT license.

use base64::Engine;
use deno_core::op2;

use x509_parser::der_parser::asn1_rs::Any;
use x509_parser::der_parser::asn1_rs::Tag;
use x509_parser::der_parser::oid::Oid;
pub use x509_parser::error::X509Error;
use x509_parser::extensions;
use x509_parser::oid_registry::OID_PKIX_AUTHORITY_INFO_ACCESS;
use x509_parser::oid_registry::OID_X509_EXT_AUTHORITY_KEY_IDENTIFIER;
use x509_parser::oid_registry::OID_X509_EXT_KEY_USAGE;
use x509_parser::oid_registry::OID_X509_EXT_SUBJECT_KEY_IDENTIFIER;
use x509_parser::pem;
use x509_parser::prelude::*;

use super::keys::AsymmetricPublicKey;
use super::KeyObjectHandle;

use std::ops::Deref;
use yoke::Yoke;
use yoke::Yokeable;

use digest::Digest;

enum CertificateSources {
  Der(Box<[u8]>),
  Pem(pem::Pem),
}

#[derive(Yokeable)]
struct CertificateView<'a> {
  cert: X509Certificate<'a>,
}

pub(crate) struct Certificate {
  inner: Yoke<CertificateView<'static>, Box<CertificateSources>>,
}

impl deno_core::GarbageCollected for Certificate {}

impl Certificate {
  fn fingerprint<D: Digest>(&self) -> Option<String> {
    if let CertificateSources::Pem(pem) = self.inner.backing_cart().as_ref() {
      let mut hasher = D::new();
      hasher.update(&pem.contents);
      let bytes = hasher.finalize();
      // OpenSSL returns colon separated upper case hex values.
      let mut hex = String::with_capacity(bytes.len() * 2);
      for byte in bytes {
        hex.push_str(&format!("{:02X}:", byte));
      }
      hex.pop();
      Some(hex)
    } else {
      None
    }
  }
}

impl<'a> Deref for CertificateView<'a> {
  type Target = X509Certificate<'a>;

  fn deref(&self) -> &Self::Target {
    &self.cert
  }
}

#[op2]
#[cppgc]
pub fn op_node_x509_parse(
  #[buffer] buf: &[u8],
) -> Result<Certificate, X509Error> {
  let source = match pem::parse_x509_pem(buf) {
    Ok((_, pem)) => CertificateSources::Pem(pem),
    Err(_) => CertificateSources::Der(buf.to_vec().into_boxed_slice()),
  };

  let inner =
    Yoke::<CertificateView<'static>, Box<CertificateSources>>::try_attach_to_cart(
      Box::new(source),
      |source| {
        let cert = match source {
          CertificateSources::Pem(pem) => pem.parse_x509()?,
          CertificateSources::Der(buf) => {
            X509Certificate::from_der(buf).map(|(_, cert)| cert)?
          }
        };
        Ok::<_, X509Error>(CertificateView { cert })
      },
    )?;

  Ok(Certificate { inner })
}

#[op2(fast)]
pub fn op_node_x509_ca(#[cppgc] cert: &Certificate) -> bool {
  let cert = cert.inner.get().deref();
  cert.is_ca()
}

#[op2(fast)]
pub fn op_node_x509_check_email(
  #[cppgc] cert: &Certificate,
  #[string] email: &str,
) -> bool {
  let cert = cert.inner.get().deref();
  let subject = cert.subject();
  if subject
    .iter_email()
    .any(|e| e.as_str().unwrap_or("") == email)
  {
    return true;
  }

  let subject_alt = cert
    .extensions()
    .iter()
    .find(|e| e.oid == x509_parser::oid_registry::OID_X509_EXT_SUBJECT_ALT_NAME)
    .and_then(|e| match e.parsed_extension() {
      extensions::ParsedExtension::SubjectAlternativeName(s) => Some(s),
      _ => None,
    });

  if let Some(subject_alt) = subject_alt {
    for name in &subject_alt.general_names {
      if let extensions::GeneralName::RFC822Name(n) = name {
        if *n == email {
          return true;
        }
      }
    }
  }

  false
}

#[op2]
#[string]
pub fn op_node_x509_fingerprint(#[cppgc] cert: &Certificate) -> Option<String> {
  cert.fingerprint::<sha1::Sha1>()
}

#[op2]
#[string]
pub fn op_node_x509_fingerprint256(
  #[cppgc] cert: &Certificate,
) -> Option<String> {
  cert.fingerprint::<sha2::Sha256>()
}

#[op2]
#[string]
pub fn op_node_x509_fingerprint512(
  #[cppgc] cert: &Certificate,
) -> Option<String> {
  cert.fingerprint::<sha2::Sha512>()
}

#[op2]
#[string]
pub fn op_node_x509_get_issuer(
  #[cppgc] cert: &Certificate,
) -> Result<String, X509Error> {
  let cert = cert.inner.get().deref();
  x509name_to_string(cert.issuer(), oid_registry())
}

#[op2]
#[string]
pub fn op_node_x509_get_subject(
  #[cppgc] cert: &Certificate,
) -> Result<String, X509Error> {
  let cert = cert.inner.get().deref();
  x509name_to_string(cert.subject(), oid_registry())
}

#[op2]
#[cppgc]
pub fn op_node_x509_public_key(
  #[cppgc] cert: &Certificate,
) -> Result<KeyObjectHandle, super::keys::X509PublicKeyError> {
  let cert = cert.inner.get().deref();
  let public_key = &cert.tbs_certificate.subject_pki;

  KeyObjectHandle::new_x509_public_key(public_key)
}

// Attempt to convert attribute to string. If type is not a string, return value is the hex
// encoding of the attribute value
fn attribute_value_to_string(
  attr: &Any,
  _attr_type: &Oid,
) -> Result<String, X509Error> {
  // TODO: replace this with helper function, when it is added to asn1-rs
  match attr.tag() {
    Tag::NumericString
    | Tag::BmpString
    | Tag::VisibleString
    | Tag::PrintableString
    | Tag::GeneralString
    | Tag::ObjectDescriptor
    | Tag::GraphicString
    | Tag::T61String
    | Tag::VideotexString
    | Tag::Utf8String
    | Tag::Ia5String => {
      let s = core::str::from_utf8(attr.data)
        .map_err(|_| X509Error::InvalidAttributes)?;
      Ok(s.to_owned())
    }
    _ => {
      // type is not a string, get slice and convert it to base64
      Ok(data_encoding::HEXUPPER.encode(attr.as_bytes()))
    }
  }
}

fn x509name_to_string(
  name: &X509Name,
  oid_registry: &oid_registry::OidRegistry,
) -> Result<String, x509_parser::error::X509Error> {
  // Lifted from https://github.com/rusticata/x509-parser/blob/4d618c2ed6b1fc102df16797545895f7c67ee0fe/src/x509.rs#L543-L566
  // since it's a private function (Copyright 2017 Pierre Chifflier)
  name.iter_rdn().try_fold(String::new(), |acc, rdn| {
    rdn
      .iter()
      .try_fold(String::new(), |acc2, attr| {
        let val_str =
          attribute_value_to_string(attr.attr_value(), attr.attr_type())?;
        // look ABBREV, and if not found, use shortname
        let abbrev = match oid2abbrev(attr.attr_type(), oid_registry) {
          Ok(s) => String::from(s),
          _ => format!("{:?}", attr.attr_type()),
        };
        let rdn = format!("{}={}", abbrev, val_str);
        match acc2.len() {
          0 => Ok(rdn),
          _ => Ok(acc2 + " + " + rdn.as_str()),
        }
      })
      .map(|v| match acc.len() {
        0 => v,
        _ => acc + "\n" + v.as_str(),
      })
  })
}

#[op2]
#[string]
pub fn op_node_x509_get_valid_from(#[cppgc] cert: &Certificate) -> String {
  let cert = cert.inner.get().deref();
  cert.validity().not_before.to_string()
}

#[op2]
#[string]
pub fn op_node_x509_get_valid_to(#[cppgc] cert: &Certificate) -> String {
  let cert = cert.inner.get().deref();
  cert.validity().not_after.to_string()
}

#[op2]
#[string]
pub fn op_node_x509_get_serial_number(#[cppgc] cert: &Certificate) -> String {
  let cert = cert.inner.get().deref();
  let mut s = cert.serial.to_str_radix(16);
  s.make_ascii_uppercase();
  s
}

#[op2(fast)]
pub fn op_node_x509_key_usage(#[cppgc] cert: &Certificate) -> u16 {
  let cert = cert.inner.get().deref();
  let key_usage = cert
    .extensions()
    .iter()
    .find(|e| e.oid == x509_parser::oid_registry::OID_X509_EXT_KEY_USAGE)
    .and_then(|e| match e.parsed_extension() {
      extensions::ParsedExtension::KeyUsage(k) => Some(k),
      _ => None,
    });

  key_usage.map(|k| k.flags).unwrap_or(0)
}

#[op2]
#[string]
pub fn op_node_x509_to_string(#[cppgc] cert: &Certificate) -> String {
  let der_bytes = match cert.inner.backing_cart().as_ref() {
    CertificateSources::Pem(pem) => &pem.contents,
    CertificateSources::Der(der) => der.as_ref(),
  };

  let b64 = base64::engine::general_purpose::STANDARD.encode(der_bytes);
  let mut pem_str = String::from("-----BEGIN CERTIFICATE-----\n");
  for chunk in b64.as_bytes().chunks(64) {
    pem_str.push_str(std::str::from_utf8(chunk).unwrap());
    pem_str.push('\n');
  }
  pem_str.push_str("-----END CERTIFICATE-----\n");
  pem_str
}

#[op2]
#[buffer]
pub fn op_node_x509_get_raw(#[cppgc] cert: &Certificate) -> Box<[u8]> {
  match cert.inner.backing_cart().as_ref() {
    CertificateSources::Pem(pem) => pem.contents.clone().into_boxed_slice(),
    CertificateSources::Der(der) => der.clone(),
  }
}

// OIDs of the public key algorithms a certificate can be signed with.
const OID_KEY_RSA: &str = "1.2.840.113549.1.1.1";
const OID_KEY_DSA: &str = "1.2.840.10040.4.1";
const OID_KEY_EC: &str = "1.2.840.10045.2.1";
const OID_KEY_ED25519: &str = "1.3.101.112";

#[derive(Clone, Copy, PartialEq, Eq)]
enum X509SignatureKind {
  RsaPkcs1v15,
  Dsa,
  Ecdsa,
  Ed25519,
}

impl X509SignatureKind {
  /// OID of the public key algorithm that produces this kind of signature.
  fn key_algorithm(self) -> &'static str {
    match self {
      X509SignatureKind::RsaPkcs1v15 => OID_KEY_RSA,
      X509SignatureKind::Dsa => OID_KEY_DSA,
      X509SignatureKind::Ecdsa => OID_KEY_EC,
      X509SignatureKind::Ed25519 => OID_KEY_ED25519,
    }
  }
}

/// Maps a certificate signature algorithm OID to its signature scheme and
/// digest. Ed25519 signs the message itself, so it has no digest.
fn x509_signature_algorithm(
  oid: &str,
) -> Option<(X509SignatureKind, Option<&'static str>)> {
  use X509SignatureKind::*;

  let algorithm = match oid {
    // sha1WithRSAEncryption, sha1WithRSASignature (OIW)
    "1.2.840.113549.1.1.5" | "1.3.14.3.2.29" => (RsaPkcs1v15, Some("sha1")),
    "1.2.840.113549.1.1.14" => (RsaPkcs1v15, Some("sha224")),
    "1.2.840.113549.1.1.11" => (RsaPkcs1v15, Some("sha256")),
    "1.2.840.113549.1.1.12" => (RsaPkcs1v15, Some("sha384")),
    "1.2.840.113549.1.1.13" => (RsaPkcs1v15, Some("sha512")),
    "1.2.840.10040.4.3" => (Dsa, Some("sha1")),
    "2.16.840.1.101.3.4.3.1" => (Dsa, Some("sha224")),
    "2.16.840.1.101.3.4.3.2" => (Dsa, Some("sha256")),
    "1.2.840.10045.4.1" => (Ecdsa, Some("sha1")),
    "1.2.840.10045.4.3.1" => (Ecdsa, Some("sha224")),
    "1.2.840.10045.4.3.2" => (Ecdsa, Some("sha256")),
    "1.2.840.10045.4.3.3" => (Ecdsa, Some("sha384")),
    "1.2.840.10045.4.3.4" => (Ecdsa, Some("sha512")),
    "1.3.101.112" => (Ed25519, None),
    _ => return None,
  };
  Some(algorithm)
}

fn x509_digest(digest_type: &str, data: &[u8]) -> Option<Vec<u8>> {
  let digest = match digest_type {
    "sha1" => sha1::Sha1::digest(data).to_vec(),
    "sha224" => sha2::Sha224::digest(data).to_vec(),
    "sha256" => sha2::Sha256::digest(data).to_vec(),
    "sha384" => sha2::Sha384::digest(data).to_vec(),
    "sha512" => sha2::Sha512::digest(data).to_vec(),
    _ => return None,
  };
  Some(digest)
}

fn get_extension<'a, 'b>(
  cert: &'b X509Certificate<'a>,
  oid: &Oid,
) -> Option<&'b ParsedExtension<'a>> {
  cert
    .extensions()
    .iter()
    .find(|e| e.oid == *oid)
    .map(|e| e.parsed_extension())
}

/// Returns whether the certificate was signed by the given public key, like
/// OpenSSL's `X509_verify` (used by Node.js). Unsupported signature
/// algorithms (e.g. RSA-PSS) and keys of the wrong type yield `false`.
#[op2(fast)]
pub fn op_node_x509_verify(
  #[cppgc] cert: &Certificate,
  #[cppgc] key: &KeyObjectHandle,
) -> bool {
  let Some(public_key) = key.as_public_key() else {
    return false;
  };

  let cert = cert.inner.get().deref();
  // The signature covers the DER encoded TBSCertificate, including its
  // SEQUENCE header.
  let tbs: &[u8] = cert.tbs_certificate.as_ref();
  let signature: &[u8] = &cert.signature_value.data;
  let sig_alg = cert.signature_algorithm.algorithm.to_id_string();

  let Some((kind, digest_type)) = x509_signature_algorithm(&sig_alg) else {
    return false;
  };

  let key_matches = match &*public_key {
    AsymmetricPublicKey::Rsa(_) => kind == X509SignatureKind::RsaPkcs1v15,
    AsymmetricPublicKey::Dsa(_) => kind == X509SignatureKind::Dsa,
    AsymmetricPublicKey::Ec(_) => kind == X509SignatureKind::Ecdsa,
    AsymmetricPublicKey::Ed25519(_) => kind == X509SignatureKind::Ed25519,
    _ => false,
  };
  if !key_matches {
    return false;
  }

  if let AsymmetricPublicKey::Ed25519(key) = &*public_key {
    return match ed25519_dalek::Signature::from_slice(signature) {
      Ok(signature) => key.verify_strict(tbs, &signature).is_ok(),
      Err(_) => false,
    };
  }

  let Some(digest_type) = digest_type else {
    return false;
  };
  let Some(digest) = x509_digest(digest_type, tbs) else {
    return false;
  };

  // X.509 stores (EC)DSA signatures DER encoded, which is what
  // `verify_prehashed` expects when `dsa_signature_encoding` is 0.
  key
    .verify_prehashed(digest_type, &digest, signature, None, 0)
    .unwrap_or(false)
}

/// Returns whether `other` could have issued the certificate, like OpenSSL's
/// `X509_check_issued` (used by Node.js). As in Node.js, the signature is not
/// verified; `verify(issuer.publicKey)` does that.
#[op2(fast)]
pub fn op_node_x509_check_issued(
  #[cppgc] cert: &Certificate,
  #[cppgc] other: &Certificate,
) -> bool {
  let cert = cert.inner.get().deref();
  let other = other.inner.get().deref();

  // 1. The issuer of `cert` must be the subject of `other`.
  if cert.issuer().as_raw() != other.subject().as_raw() {
    return false;
  }

  // 2. If `cert` has an Authority Key Identifier with a key identifier, it
  //    must match the Subject Key Identifier of `other`.
  let aki = get_extension(cert, &OID_X509_EXT_AUTHORITY_KEY_IDENTIFIER);
  if let Some(ParsedExtension::AuthorityKeyIdentifier(aki)) = aki {
    if let Some(key_id) = &aki.key_identifier {
      let ski = get_extension(other, &OID_X509_EXT_SUBJECT_KEY_IDENTIFIER);
      match ski {
        Some(ParsedExtension::SubjectKeyIdentifier(ski)) => {
          if key_id.0 != ski.0 {
            return false;
          }
        }
        _ => return false,
      }
    }
  }

  // 3. If `other` has a Key Usage extension, it must allow keyCertSign.
  let key_usage = get_extension(other, &OID_X509_EXT_KEY_USAGE);
  if let Some(ParsedExtension::KeyUsage(key_usage)) = key_usage {
    if !key_usage.key_cert_sign() {
      return false;
    }
  }

  // 4. The key type of `other` must match the signature algorithm of `cert`.
  let sig_alg = cert.signature_algorithm.algorithm.to_id_string();
  if let Some((kind, _)) = x509_signature_algorithm(&sig_alg) {
    let key_alg = other.public_key().algorithm.algorithm.to_id_string();
    if key_alg != kind.key_algorithm() {
      return false;
    }
  }

  true
}

/// Returns the Authority Information Access extension in the Node.js format
/// (one `<method> - <type>:<value>` entry per line), or `None` if absent.
#[op2]
#[string]
pub fn op_node_x509_get_info_access(
  #[cppgc] cert: &Certificate,
) -> Option<String> {
  let cert = cert.inner.get().deref();

  let aia = match get_extension(cert, &OID_PKIX_AUTHORITY_INFO_ACCESS) {
    Some(ParsedExtension::AuthorityInfoAccess(aia)) => aia,
    _ => return None,
  };

  let mut entries = Vec::with_capacity(aia.accessdescs.len());
  for desc in aia.iter() {
    let method = match desc.access_method.to_id_string().as_str() {
      "1.3.6.1.5.5.7.48.1" => "OCSP".to_string(),
      "1.3.6.1.5.5.7.48.2" => "CA Issuers".to_string(),
      method_oid => method_oid.to_string(),
    };
    let location = match &desc.access_location {
      GeneralName::URI(uri) => format!("URI:{}", uri),
      GeneralName::DNSName(dns) => format!("DNS:{}", dns),
      GeneralName::RFC822Name(email) => format!("email:{}", email),
      _ => continue,
    };
    entries.push(format!("{} - {}", method, location));
  }

  if entries.is_empty() {
    None
  } else {
    Some(entries.join("\n"))
  }
}
