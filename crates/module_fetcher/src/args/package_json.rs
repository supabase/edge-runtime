// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use deno_core::anyhow::bail;
use deno_core::error::AnyError;
use deno_npm::registry::parse_dep_entry_name_and_raw_version;
use deno_npm::registry::PackageDepNpmSchemeValueParseError;
use deno_semver::npm::NpmPackageReq;
use deno_semver::npm::NpmVersionReqSpecifierParseError;
use deno_semver::VersionReq;
use sb_node::PackageJson;
use thiserror::Error;

#[derive(Debug, Error, Clone)]
pub enum PackageJsonDepValueParseError {
    #[error(transparent)]
    SchemeValue(#[from] PackageDepNpmSchemeValueParseError),
    #[error(transparent)]
    Specifier(#[from] NpmVersionReqSpecifierParseError),
    #[error("Not implemented scheme '{scheme}'")]
    Unsupported { scheme: String },
}

pub type PackageJsonDeps = BTreeMap<String, Result<NpmPackageReq, PackageJsonDepValueParseError>>;

#[derive(Debug, Default)]
pub struct PackageJsonDepsProvider(Option<PackageJsonDeps>);

impl PackageJsonDepsProvider {
    pub fn new(deps: Option<PackageJsonDeps>) -> Self {
        Self(deps)
    }

    pub fn deps(&self) -> Option<&PackageJsonDeps> {
        self.0.as_ref()
    }

    pub fn reqs(&self) -> Vec<&NpmPackageReq> {
        match &self.0 {
            Some(deps) => {
                let mut package_reqs = deps
                    .values()
                    .filter_map(|r| r.as_ref().ok())
                    .collect::<Vec<_>>();
                package_reqs.sort(); // deterministic resolution
                package_reqs
            }
            None => Vec::new(),
        }
    }
}

/// Gets an application level package.json's npm package requirements.
///
/// Note that this function is not general purpose. It is specifically for
/// parsing the application level package.json that the user has control
/// over. This is a design limitation to allow mapping these dependency
/// entries to npm specifiers which can then be used in the resolver.
pub fn get_local_package_json_version_reqs(package_json: &PackageJson) -> PackageJsonDeps {
    fn parse_entry(key: &str, value: &str) -> Result<NpmPackageReq, PackageJsonDepValueParseError> {
        if value.starts_with("workspace:")
            || value.starts_with("file:")
            || value.starts_with("git:")
            || value.starts_with("http:")
            || value.starts_with("https:")
        {
            return Err(PackageJsonDepValueParseError::Unsupported {
                scheme: value.split(':').next().unwrap().to_string(),
            });
        }
        let (name, version_req) = parse_dep_entry_name_and_raw_version(key, value)
            .map_err(PackageJsonDepValueParseError::SchemeValue)?;

        let result = VersionReq::parse_from_specifier(version_req);
        match result {
            Ok(version_req) => Ok(NpmPackageReq {
                name: name.to_string(),
                version_req,
            }),
            Err(err) => Err(PackageJsonDepValueParseError::Specifier(err)),
        }
    }

    fn insert_deps(deps: Option<&HashMap<String, String>>, result: &mut PackageJsonDeps) {
        if let Some(deps) = deps {
            for (key, value) in deps {
                result.insert(key.to_string(), parse_entry(key, value));
            }
        }
    }

    let deps = package_json.dependencies.as_ref();
    let dev_deps = package_json.dev_dependencies.as_ref();
    let mut result = BTreeMap::new();

    // insert the dev dependencies first so the dependencies will
    // take priority and overwrite any collisions
    insert_deps(dev_deps, &mut result);
    insert_deps(deps, &mut result);

    result
}

/// Attempts to discover the package.json file, maybe stopping when it
/// reaches the specified `maybe_stop_at` directory.
pub fn discover_from(
    start: &Path,
    maybe_stop_at: Option<PathBuf>,
) -> Result<Option<PackageJson>, AnyError> {
    const PACKAGE_JSON_NAME: &str = "package.json";

    // note: ancestors() includes the `start` path
    for ancestor in start.ancestors() {
        let path = ancestor.join(PACKAGE_JSON_NAME);

        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                if let Some(stop_at) = maybe_stop_at.as_ref() {
                    if ancestor == stop_at {
                        break;
                    }
                }
                continue;
            }
            Err(err) => bail!(
                "Error loading package.json at {}. {:#}",
                path.display(),
                err
            ),
        };

        let package_json = PackageJson::load_from_string(path.clone(), source)?;
        log::debug!("package.json file found at '{}'", path.display());
        return Ok(Some(package_json));
    }

    log::debug!("No package.json file found");
    Ok(None)
}
