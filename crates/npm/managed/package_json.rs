// Copyright 2018-2024 the Deno authors. All rights reserved. MIT license.
use deno_npm::registry::parse_dep_entry_name_and_raw_version;
use deno_semver::package::PackageReq;
use deno_semver::VersionReq;
use deno_semver::VersionReqSpecifierParseError;
use indexmap::IndexMap;
use sb_node::PackageJson;
use thiserror::Error;

#[derive(Debug, Error, Clone)]
pub enum PackageJsonDepValueParseError {
    #[error(transparent)]
    Specifier(#[from] VersionReqSpecifierParseError),
    #[error("Not implemented scheme '{scheme}'")]
    Unsupported { scheme: String },
}

pub type PackageJsonDeps = IndexMap<String, Result<PackageReq, PackageJsonDepValueParseError>>;

#[derive(Debug, Default)]
pub struct PackageJsonDepsProvider(Option<PackageJsonDeps>);

impl PackageJsonDepsProvider {
    pub fn new(deps: Option<PackageJsonDeps>) -> Self {
        Self(deps)
    }

    pub fn deps(&self) -> Option<&PackageJsonDeps> {
        self.0.as_ref()
    }

    pub fn reqs(&self) -> Option<Vec<&PackageReq>> {
        match &self.0 {
            Some(deps) => {
                let mut package_reqs = deps
                    .values()
                    .filter_map(|r| r.as_ref().ok())
                    .collect::<Vec<_>>();
                package_reqs.sort(); // deterministic resolution
                Some(package_reqs)
            }
            None => None,
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
    fn parse_entry(key: &str, value: &str) -> Result<PackageReq, PackageJsonDepValueParseError> {
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
        let (name, version_req) = parse_dep_entry_name_and_raw_version(key, value);
        let result = VersionReq::parse_from_specifier(version_req);
        match result {
            Ok(version_req) => Ok(PackageReq {
                name: name.to_string(),
                version_req,
            }),
            Err(err) => Err(PackageJsonDepValueParseError::Specifier(err)),
        }
    }

    fn insert_deps(deps: Option<&IndexMap<String, String>>, result: &mut PackageJsonDeps) {
        if let Some(deps) = deps {
            for (key, value) in deps {
                result
                    .entry(key.to_string())
                    .or_insert_with(|| parse_entry(key, value));
            }
        }
    }

    let deps = package_json.dependencies.as_ref();
    let dev_deps = package_json.dev_dependencies.as_ref();
    let mut result = IndexMap::new();

    // favors the deps over dev_deps
    insert_deps(deps, &mut result);
    insert_deps(dev_deps, &mut result);

    result
}
