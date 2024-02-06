use deno_semver::package::PackageReq;
use deno_semver::VersionReqSpecifierParseError;
use sb_npm::package_json::{PackageJsonDepValueParseError, PackageJsonDeps};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
enum SerializablePackageJsonDepValueParseError {
    Specifier(String),
    Unsupported { scheme: String },
}

impl SerializablePackageJsonDepValueParseError {
    pub fn from_err(err: PackageJsonDepValueParseError) -> Self {
        match err {
            PackageJsonDepValueParseError::Specifier(err) => {
                Self::Specifier(err.source.to_string())
            }
            PackageJsonDepValueParseError::Unsupported { scheme } => Self::Unsupported { scheme },
        }
    }

    pub fn into_err(self) -> PackageJsonDepValueParseError {
        match self {
            SerializablePackageJsonDepValueParseError::Specifier(source) => {
                PackageJsonDepValueParseError::Specifier(VersionReqSpecifierParseError {
                    source: monch::ParseErrorFailureError::new(source),
                })
            }
            SerializablePackageJsonDepValueParseError::Unsupported { scheme } => {
                PackageJsonDepValueParseError::Unsupported { scheme }
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct SerializablePackageJsonDeps(
    BTreeMap<String, Result<PackageReq, SerializablePackageJsonDepValueParseError>>,
);

impl SerializablePackageJsonDeps {
    pub fn from_deps(deps: PackageJsonDeps) -> Self {
        Self(
            deps.into_iter()
                .map(|(name, req)| {
                    let res = req.map_err(SerializablePackageJsonDepValueParseError::from_err);
                    (name, res)
                })
                .collect(),
        )
    }

    pub fn into_deps(self) -> PackageJsonDeps {
        self.0
            .into_iter()
            .map(|(name, res)| (name, res.map_err(|err| err.into_err())))
            .collect()
    }
}

#[derive(Deserialize, Serialize)]
pub struct Metadata {
    pub ca_stores: Option<Vec<String>>,
    pub ca_data: Option<Vec<u8>>,
    pub unsafely_ignore_certificate_errors: Option<Vec<String>>,
    pub package_json_deps: Option<SerializablePackageJsonDeps>,
}
