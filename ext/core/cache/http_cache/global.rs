// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;

use deno_core::error::AnyError;
use deno_core::serde_json;
use deno_core::url::Url;
use thiserror::Error;

use crate::cache::CACHE_PERM;
use crate::util;
use crate::util::fs::atomic_write_file;
use crate::util::http_util::HeadersMap;

use super::common::base_url_to_filename_parts;
use super::common::read_file_bytes;
use super::CachedUrlMetadata;
use super::HttpCache;
use super::HttpCacheItemKey;

#[derive(Debug, Error)]
#[error("Can't convert url (\"{}\") to filename.", .url)]
pub struct UrlToFilenameConversionError {
    pub(super) url: String,
}

/// Turn provided `url` into a hashed filename.
/// URLs can contain a lot of characters that cannot be used
/// in filenames (like "?", "#", ":"), so in order to cache
/// them properly they are deterministically hashed into ASCII
/// strings.
pub fn url_to_filename(url: &Url) -> Result<PathBuf, UrlToFilenameConversionError> {
    if let Some(mut cache_filename) = base_url_to_filename(url) {
        let mut rest_str = url.path().to_string();
        if let Some(query) = url.query() {
            rest_str.push('?');
            rest_str.push_str(query);
        }
        // NOTE: fragment is omitted on purpose - it's not taken into
        // account when caching - it denotes parts of webpage, which
        // in case of static resources doesn't make much sense
        let hashed_filename = util::checksum::gen(&[rest_str.as_bytes()]);
        cache_filename.push(hashed_filename);
        Ok(cache_filename)
    } else {
        Err(UrlToFilenameConversionError {
            url: url.to_string(),
        })
    }
}

// Turn base of url (scheme, hostname, port) into a valid filename.
/// This method replaces port part with a special string token (because
/// ":" cannot be used in filename on some platforms).
/// Ex: $DENO_DIR/deps/https/deno.land/
fn base_url_to_filename(url: &Url) -> Option<PathBuf> {
    base_url_to_filename_parts(url, "_PORT").map(|parts| {
        let mut out = PathBuf::new();
        for part in parts {
            out.push(part);
        }
        out
    })
}

#[derive(Debug)]
pub struct GlobalHttpCache(PathBuf);

impl GlobalHttpCache {
    fn get_cache_filepath(&self, url: &Url) -> Result<PathBuf, AnyError> {
        Ok(self.0.join(url_to_filename(url)?))
    }

    #[inline]
    fn key_file_path<'a>(&self, key: &'a HttpCacheItemKey) -> &'a PathBuf {
        // The key file path is always set for the global cache because
        // the file will always exist, unlike the local cache, which won't
        // have this for redirects.
        key.file_path.as_ref().unwrap()
    }
}

impl HttpCache for GlobalHttpCache {
    fn cache_item_key<'a>(&self, url: &'a Url) -> Result<HttpCacheItemKey<'a>, AnyError> {
        Ok(HttpCacheItemKey {
            #[cfg(debug_assertions)]
            is_local_key: false,
            url,
            file_path: Some(self.get_cache_filepath(url)?),
        })
    }

    fn contains(&self, url: &Url) -> bool {
        if let Ok(cache_filepath) = self.get_cache_filepath(url) {
            cache_filepath.is_file()
        } else {
            false
        }
    }

    fn read_modified_time(&self, key: &HttpCacheItemKey) -> Result<Option<SystemTime>, AnyError> {
        #[cfg(debug_assertions)]
        debug_assert!(!key.is_local_key);

        match std::fs::metadata(self.key_file_path(key)) {
            Ok(metadata) => Ok(Some(metadata.modified()?)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn set(&self, url: &Url, headers: HeadersMap, content: &[u8]) -> Result<(), AnyError> {
        let cache_filepath = self.get_cache_filepath(url)?;
        // Cache content
        atomic_write_file(&cache_filepath, content, CACHE_PERM)?;

        let metadata = CachedUrlMetadata {
            time: SystemTime::now(),
            url: url.to_string(),
            headers,
        };
        write_metadata(&cache_filepath, &metadata)?;

        Ok(())
    }

    fn read_file_bytes(&self, key: &HttpCacheItemKey) -> Result<Option<Vec<u8>>, AnyError> {
        #[cfg(debug_assertions)]
        debug_assert!(!key.is_local_key);

        Ok(read_file_bytes(self.key_file_path(key))?)
    }

    fn read_metadata(&self, key: &HttpCacheItemKey) -> Result<Option<CachedUrlMetadata>, AnyError> {
        #[cfg(debug_assertions)]
        debug_assert!(!key.is_local_key);

        match read_metadata(self.key_file_path(key))? {
            Some(metadata) => Ok(Some(metadata)),
            None => Ok(None),
        }
    }
}

fn read_metadata(path: &Path) -> Result<Option<CachedUrlMetadata>, AnyError> {
    let path = path.with_extension("metadata.json");
    match read_file_bytes(&path)? {
        Some(metadata) => Ok(Some(serde_json::from_slice(&metadata)?)),
        None => Ok(None),
    }
}

fn write_metadata(path: &Path, meta_data: &CachedUrlMetadata) -> Result<(), AnyError> {
    let path = path.with_extension("metadata.json");
    let json = serde_json::to_string_pretty(meta_data)?;
    atomic_write_file(&path, json, CACHE_PERM)?;
    Ok(())
}
