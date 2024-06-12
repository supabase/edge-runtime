use crate::emitter::EmitterFactory;
use crate::errors::EszipError;
use crate::graph_util::{create_eszip_from_graph_raw, create_graph};
use anyhow::{bail, Context};
use deno_ast::MediaType;
use deno_core::error::AnyError;
use deno_core::futures::io::{AllowStdIo, BufReader};
use deno_core::url::Url;
use deno_core::{serde_json, FastString, JsBuffer, ModuleSpecifier};
use deno_fs::{FileSystem, RealFs};
use deno_npm::NpmSystemInfo;
use eszip::v2::{EszipV2Module, EszipV2Modules, EszipV2SourceSlot};
use eszip::{EszipV2, Module, ModuleKind, ParseError};
use futures::future::OptionFuture;
use futures::{AsyncReadExt, AsyncSeekExt};
use glob::glob;
use log::{error, warn};
use sb_eszip_shared::{
    AsyncEszipDataRead, SOURCE_CODE_ESZIP_KEY, STATIC_FILES_ESZIP_KEY, SUPABASE_ESZIP_VERSION,
    SUPABASE_ESZIP_VERSION_KEY, VFS_ESZIP_KEY,
};
use sb_fs::{build_vfs, VfsOpts};
use sb_npm::InnerCliNpmResolverRef;
use scopeguard::ScopeGuard;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::fs::{create_dir_all, File};
use std::io::{Cursor, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

mod eszip_parse;

pub mod emitter;
pub mod errors;
pub mod eszip_migrate;
pub mod graph_fs;
pub mod graph_resolver;
pub mod graph_util;
pub mod import_map;
pub mod jsr;
pub mod jsx_util;

pub const STATIC_FS_PREFIX: &str = "mnt/data";

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecoratorType {
    /// Use TC39 Decorators Proposal - https://github.com/tc39/proposal-decorators
    Tc39,
    /// Use TypeScript experimental decorators.
    Typescript,
    /// Use TypeScript experimental decorators. It also emits metadata.
    TypescriptWithMetadata,
}

impl Default for DecoratorType {
    fn default() -> Self {
        Self::Typescript
    }
}

impl DecoratorType {
    fn is_use_decorators_proposal(self) -> bool {
        matches!(self, Self::Tc39)
    }

    fn is_use_ts_decorators(self) -> bool {
        matches!(self, Self::Typescript | Self::TypescriptWithMetadata)
    }

    fn is_emit_metadata(self) -> bool {
        matches!(self, Self::TypescriptWithMetadata)
    }
}

#[derive(Debug)]
pub enum EszipPayloadKind {
    JsBufferKind(JsBuffer),
    VecKind(Vec<u8>),
    Eszip(EszipV2),
}

async fn read_u32<R: futures::io::AsyncRead + Unpin>(reader: &mut R) -> Result<u32, ParseError> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf).await?;
    Ok(u32::from_be_bytes(buf))
}

#[derive(Debug)]
pub struct LazyLoadableEszip {
    eszip: EszipV2,
    maybe_data_section: Option<Arc<EszipDataSection>>,
}

impl std::ops::Deref for LazyLoadableEszip {
    type Target = EszipV2;

    fn deref(&self) -> &Self::Target {
        &self.eszip
    }
}

impl std::ops::DerefMut for LazyLoadableEszip {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.eszip
    }
}

impl Clone for LazyLoadableEszip {
    fn clone(&self) -> Self {
        Self {
            eszip: EszipV2 {
                modules: self.eszip.modules.clone(),
                npm_snapshot: None,
            },
            maybe_data_section: self.maybe_data_section.clone(),
        }
    }
}

impl AsyncEszipDataRead for LazyLoadableEszip {
    fn ensure_module(&self, specifier: &str) -> Option<Module> {
        let module = self.ensure_data(specifier)?;

        if module.kind == ModuleKind::Jsonc {
            return None;
        }

        Some(module)
    }

    fn ensure_import_map(&self, specifier: &str) -> Option<Module> {
        let module = self.ensure_data(specifier)?;

        if module.kind == ModuleKind::JavaScript {
            return None;
        }

        Some(module)
    }
}

impl LazyLoadableEszip {
    fn new(eszip: EszipV2, maybe_data_section: Option<Arc<EszipDataSection>>) -> Self {
        Self {
            eszip,
            maybe_data_section,
        }
    }

    pub fn ensure_data(&self, specifier: &str) -> Option<Module> {
        let module = self
            .get_module(specifier)
            .or_else(|| self.get_import_map(specifier))?;

        if let Some(section) = self.maybe_data_section.clone() {
            let specifier = module.specifier.clone();

            drop(tokio::spawn(async move {
                match section.read_data_section_by_specifier(&specifier).await {
                    Ok(_) => {}
                    Err(err) => {
                        error!("failed to read module data from the data section: {}", err);
                    }
                }
            }));
        }

        Some(module)
    }

    pub async fn ensure_read_all(&mut self) -> Result<(), ParseError> {
        if let Some(section) = self.maybe_data_section.take() {
            section.read_data_section_all().await
        } else {
            Ok(())
        }
    }

    pub async fn ensure_version(&self) -> Result<(), anyhow::Error> {
        let version = OptionFuture::<_>::from(
            self.ensure_module(SUPABASE_ESZIP_VERSION_KEY)
                .map(|it| async move { it.source().await }),
        )
        .await
        .flatten();

        if !matches!(version, Some(ref v) if v.as_ref() == SUPABASE_ESZIP_VERSION) {
            bail!(EszipError::UnsupportedVersion {
                expected: SUPABASE_ESZIP_VERSION,
                found: version.as_deref().map(<[u8]>::to_vec)
            });
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EszipDataLoc {
    source_offset: usize,
    source_length: usize,
    source_map_offset: usize,
    source_map_length: usize,
}

#[derive(Debug, Clone)]
pub enum EszipDataSectionMetadata {
    HasLocation(EszipDataLoc),
    PendingOrAlreadyLoaded,
}

#[derive(Debug, Clone)]
pub struct EszipDataSection {
    inner: Arc<Mutex<Cursor<Vec<u8>>>>,
    initial_offset: u64,
    modules: EszipV2Modules,
    locs_by_specifier: Arc<Mutex<Option<HashMap<String, EszipDataSectionMetadata>>>>,
    loaded_locs_by_specifier: Arc<Mutex<HashMap<String, EszipDataLoc>>>,
}

impl EszipDataSection {
    pub fn new(inner: Cursor<Vec<u8>>, initial_offset: u64, modules: EszipV2Modules) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
            initial_offset,
            modules,
            locs_by_specifier: Arc::default(),
            loaded_locs_by_specifier: Arc::default(),
        }
    }

    pub async fn read_data_section_by_specifier(
        &self,
        specifier: &str,
    ) -> Result<(), anyhow::Error> {
        let mut locs_guard = self.locs_by_specifier.lock().await;
        let locs = locs_guard.get_or_insert_with(|| {
            self.modules
                .0
                .lock()
                .unwrap()
                .iter()
                .filter_map(|(specifier, m)| {
                    let mut loc = EszipDataLoc::default();
                    let (source_slot, source_map_slot) = match m {
                        EszipV2Module::Module {
                            source, source_map, ..
                        } => (source, source_map),
                        EszipV2Module::Redirect { .. } => return None,
                    };

                    match source_slot {
                        EszipV2SourceSlot::Pending { offset, length, .. } => {
                            loc.source_offset = *offset;
                            loc.source_length = *length;
                        }

                        EszipV2SourceSlot::Ready(_) | EszipV2SourceSlot::Taken => {
                            return Some((
                                specifier.clone(),
                                EszipDataSectionMetadata::PendingOrAlreadyLoaded,
                            ));
                        }
                    }

                    if let EszipV2SourceSlot::Pending { offset, length, .. } = source_map_slot {
                        loc.source_map_offset = *offset;
                        loc.source_map_length = *length;
                    }

                    Some((
                        specifier.clone(),
                        EszipDataSectionMetadata::HasLocation(loc),
                    ))
                })
                .collect::<HashMap<_, _>>()
        });

        let Some(metadata) = locs.get_mut(specifier) else {
            bail!("given specifier does not exist in the eszip header")
        };

        let loc = match metadata {
            metadata @ &mut EszipDataSectionMetadata::HasLocation(loc) => {
                self.loaded_locs_by_specifier
                    .lock()
                    .await
                    .insert(String::from(specifier), loc);

                *metadata = EszipDataSectionMetadata::PendingOrAlreadyLoaded;
                loc
            }

            _ => return Ok(()),
        };

        drop(locs_guard);

        let mut inner = self.inner.lock().await;
        let mut io = AllowStdIo::new({
            // NOTE: 4 byte offset in the middle represents the full source length.
            inner.set_position(self.initial_offset + 4 + loc.source_offset as u64);
            inner.by_ref()
        });

        let source_bytes = {
            let wake_guard = scopeguard::guard(&self.modules, |modules| {
                Self::wake_source_slot(modules, specifier, || EszipV2SourceSlot::Taken);
            });

            let mut source_bytes = vec![0u8; loc.source_length];
            io.read_exact(&mut source_bytes).await?;

            let expected_hash = &mut [0u8; 32];
            io.read_exact(expected_hash).await?;

            let mut hasher = Sha256::new();
            hasher.update(&source_bytes);

            let actual_hash = hasher.finalize();

            if &*actual_hash != expected_hash {
                return Err(ParseError::InvalidV2SourceHash(specifier.to_string()))
                    .context("invalid source hash");
            }

            let _ = ScopeGuard::into_inner(wake_guard);

            source_bytes
        };

        Self::wake_source_slot(&self.modules, specifier, move || {
            EszipV2SourceSlot::Ready(Arc::from(source_bytes))
        });

        let source_map_bytes = 'scope: {
            if loc.source_map_length == 0 {
                break 'scope None::<Vec<u8>>;
            }

            let wake_guard = scopeguard::guard(&self.modules, |modules| {
                Self::wake_source_map_slot(modules, specifier, || EszipV2SourceSlot::Taken);
            });

            let mut source_map_bytes = vec![0u8; loc.source_map_length];
            io.read_exact(&mut source_map_bytes).await?;

            let expected_hash = &mut [0u8; 32];
            io.read_exact(expected_hash).await?;

            let mut hasher = Sha256::new();
            hasher.update(&source_map_bytes);

            let actual_hash = hasher.finalize();

            if &*actual_hash != expected_hash {
                return Err(ParseError::InvalidV2SourceHash(specifier.to_string()))
                    .context("invalid source hash");
            }

            let _ = ScopeGuard::into_inner(wake_guard);

            Some(source_map_bytes)
        };

        if let Some(bytes) = source_map_bytes {
            Self::wake_source_map_slot(&self.modules, specifier, move || {
                EszipV2SourceSlot::Ready(Arc::from(bytes))
            });
        }

        Ok(())
    }

    pub async fn read_data_section_all(self: Arc<Self>) -> Result<(), ParseError> {
        // NOTE: Below codes is roughly originated from eszip@0.60.0/src/v2.rs

        let this = Arc::into_inner(self).unwrap();
        let modules = this.modules;
        let mut loaded_locs = Arc::into_inner(this.loaded_locs_by_specifier)
            .unwrap()
            .into_inner();

        let mut inner = this.inner.try_lock_owned().unwrap();
        let mut io = AllowStdIo::new({
            inner.set_position(this.initial_offset);
            inner.by_ref()
        });

        let sources_len = read_u32(&mut io).await? as usize;
        let mut read = 0;

        let mut source_offsets = modules
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(specifier, m)| {
                if let EszipV2Module::Module {
                    source: EszipV2SourceSlot::Pending { offset, length, .. },
                    ..
                } = m
                {
                    Some((*offset, (*length, specifier.clone(), true)))
                } else {
                    loaded_locs.remove(specifier.as_str()).map(|loc| {
                        (
                            loc.source_offset,
                            (loc.source_length, specifier.clone(), false),
                        )
                    })
                }
            })
            .collect::<HashMap<_, _>>();

        let mut source_map_offsets = modules
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(specifier, m)| {
                if let EszipV2Module::Module {
                    source_map: EszipV2SourceSlot::Pending { offset, length, .. },
                    ..
                } = m
                {
                    Some((*offset, (*length, specifier.clone(), true)))
                } else {
                    loaded_locs.remove(specifier.as_str()).map(|loc| {
                        (
                            loc.source_map_offset,
                            (loc.source_map_length, specifier.clone(), false),
                        )
                    })
                }
            })
            .collect::<HashMap<_, _>>();

        while read < sources_len {
            let (length, specifier, need_load) = source_offsets
                .remove(&read)
                .ok_or(ParseError::InvalidV2SourceOffset(read))?;

            if !need_load {
                read += length + 32;

                io.seek(SeekFrom::Current((length + 32) as i64))
                    .await
                    .unwrap();

                continue;
            }

            let mut source_bytes = vec![0u8; length];
            io.read_exact(&mut source_bytes).await?;

            let expected_hash = &mut [0u8; 32];
            io.read_exact(expected_hash).await?;

            let mut hasher = Sha256::new();
            hasher.update(&source_bytes);

            let actual_hash = hasher.finalize();

            if &*actual_hash != expected_hash {
                return Err(ParseError::InvalidV2SourceHash(specifier));
            }

            read += length + 32;

            Self::wake_source_slot(&modules, &specifier, move || {
                EszipV2SourceSlot::Ready(Arc::from(source_bytes))
            });
        }

        let sources_maps_len = read_u32(&mut io).await? as usize;
        let mut read = 0;

        while read < sources_maps_len {
            let (length, specifier, need_load) = source_map_offsets
                .remove(&read)
                .ok_or(ParseError::InvalidV2SourceOffset(read))?;

            if !need_load {
                read += length + 32;

                io.seek(SeekFrom::Current((length + 32) as i64))
                    .await
                    .unwrap();

                continue;
            }

            let mut source_map_bytes = vec![0u8; length];
            io.read_exact(&mut source_map_bytes).await?;

            let expected_hash = &mut [0u8; 32];
            io.read_exact(expected_hash).await?;

            let mut hasher = Sha256::new();
            hasher.update(&source_map_bytes);

            let actual_hash = hasher.finalize();

            if &*actual_hash != expected_hash {
                return Err(ParseError::InvalidV2SourceHash(specifier));
            }

            read += length + 32;

            Self::wake_source_map_slot(&modules, &specifier, move || {
                EszipV2SourceSlot::Ready(Arc::from(source_map_bytes))
            });
        }

        Ok(())
    }

    fn wake_module_with_slot<F, G>(
        modules: &EszipV2Modules,
        specifier: &str,
        select_slot_fn: F,
        new_slot_fn: G,
    ) where
        F: for<'r> FnOnce(&'r mut EszipV2Module) -> &'r mut EszipV2SourceSlot,
        G: FnOnce() -> EszipV2SourceSlot,
    {
        let wakers = {
            let mut modules = modules.0.lock().unwrap();
            let module = modules.get_mut(specifier).expect("module not found");
            let slot = select_slot_fn(module);

            let old_slot = std::mem::replace(slot, new_slot_fn());

            match old_slot {
                EszipV2SourceSlot::Pending { wakers, .. } => wakers,
                _ => panic!("already populated source slot"),
            }
        };

        for w in wakers {
            w.wake();
        }
    }

    fn wake_source_slot<F>(modules: &EszipV2Modules, specifier: &str, new_slot_fn: F)
    where
        F: FnOnce() -> EszipV2SourceSlot,
    {
        Self::wake_module_with_slot(
            modules,
            specifier,
            |module| match module {
                EszipV2Module::Module { ref mut source, .. } => source,
                _ => panic!("invalid module type"),
            },
            new_slot_fn,
        )
    }

    fn wake_source_map_slot<F>(modules: &EszipV2Modules, specifier: &str, new_slot_fn: F)
    where
        F: FnOnce() -> EszipV2SourceSlot,
    {
        Self::wake_module_with_slot(
            modules,
            specifier,
            |module| match module {
                EszipV2Module::Module {
                    ref mut source_map, ..
                } => source_map,
                _ => panic!("invalid module type"),
            },
            new_slot_fn,
        )
    }
}

pub async fn payload_to_eszip(eszip_payload_kind: EszipPayloadKind) -> LazyLoadableEszip {
    match eszip_payload_kind {
        EszipPayloadKind::Eszip(eszip) => LazyLoadableEszip::new(eszip, None),
        _ => {
            let bytes = match eszip_payload_kind {
                EszipPayloadKind::JsBufferKind(js_buffer) => Vec::from(&*js_buffer),
                EszipPayloadKind::VecKind(vec) => vec,
                _ => unreachable!(),
            };

            let mut io = AllowStdIo::new(Cursor::new(bytes));
            let mut bufreader = BufReader::new(&mut io);

            let eszip = eszip_parse::parse_v2_header(&mut bufreader).await.unwrap();

            let initial_offset = bufreader.stream_position().await.unwrap();
            let data_section =
                EszipDataSection::new(io.into_inner(), initial_offset, eszip.modules.clone());

            LazyLoadableEszip::new(eszip, Some(Arc::new(data_section)))
        }
    }
}

pub async fn generate_binary_eszip(
    file: PathBuf,
    emitter_factory: Arc<EmitterFactory>,
    maybe_module_code: Option<FastString>,
    maybe_import_map_url: Option<String>,
) -> Result<EszipV2, AnyError> {
    let graph = create_graph(file.clone(), emitter_factory.clone(), &maybe_module_code).await;
    let mut eszip = create_eszip_from_graph_raw(graph, Some(emitter_factory.clone())).await?;

    let fs_path = file.clone();
    let source_code: Arc<str> = if let Some(code) = maybe_module_code {
        code.as_str().into()
    } else {
        let entry_content = RealFs
            .read_file_sync(fs_path.clone().as_path(), None)
            .unwrap();

        String::from_utf8(entry_content.clone())?.into()
    };

    let emit_source = emitter_factory.emitter().unwrap().emit_parsed_source(
        &ModuleSpecifier::parse(
            &Url::from_file_path(&fs_path)
                .map(|it| Cow::Owned(it.to_string()))
                .ok()
                .unwrap_or("http://localhost".into()),
        )
        .unwrap(),
        MediaType::from_path(fs_path.clone().as_path()),
        &source_code,
    )?;

    let bin_code: Arc<[u8]> = emit_source.as_bytes().into();

    let npm_res = emitter_factory.npm_resolution().await;
    let resolver = emitter_factory.npm_resolver().await;

    let (npm_vfs, _npm_files) = match resolver.clone().as_inner() {
        InnerCliNpmResolverRef::Managed(managed) => {
            let snapshot = managed.serialized_valid_snapshot_for_system(&NpmSystemInfo::default());
            if !snapshot.as_serialized().packages.is_empty() {
                let mut count = 0;
                let (root_dir, files) = build_vfs(
                    VfsOpts {
                        npm_resolver: resolver.clone(),
                        npm_registry_api: emitter_factory.npm_api().await.clone(),
                        npm_cache: emitter_factory.npm_cache().await.clone(),
                        npm_resolution: emitter_factory.npm_resolution().await.clone(),
                    },
                    |_path, _key, content| {
                        let key = format!("vfs://{}", count);

                        count += 1;
                        eszip.add_opaque_data(key.clone(), content.into());
                        key
                    },
                )?
                .into_dir_and_files();

                let snapshot =
                    npm_res.serialized_valid_snapshot_for_system(&NpmSystemInfo::default());

                eszip.add_npm_snapshot(snapshot);

                (Some(root_dir), files)
            } else {
                (None, Vec::new())
            }
        }
        InnerCliNpmResolverRef::Byonm(_) => unreachable!(),
    };

    let npm_vfs =
        rkyv::to_bytes::<_, 1024>(&npm_vfs).with_context(|| "cannot serialize vfs data")?;

    eszip.add_opaque_data(
        String::from(SUPABASE_ESZIP_VERSION_KEY),
        Arc::from(SUPABASE_ESZIP_VERSION),
    );

    eszip.add_opaque_data(
        String::from(VFS_ESZIP_KEY),
        Arc::from(npm_vfs.into_boxed_slice()),
    );

    eszip.add_opaque_data(String::from(SOURCE_CODE_ESZIP_KEY), bin_code);

    // add import map
    if emitter_factory.maybe_import_map.is_some() {
        eszip.add_import_map(
            ModuleKind::Json,
            maybe_import_map_url.unwrap(),
            Arc::from(
                emitter_factory
                    .maybe_import_map
                    .as_ref()
                    .unwrap()
                    .to_json()
                    .as_bytes(),
            ),
        );
    };

    Ok(eszip)
}

pub async fn include_glob_patterns_in_eszip(
    patterns: Vec<&str>,
    eszip: &mut EszipV2,
    prefix: Option<String>,
) {
    let mut static_files: Vec<String> = vec![];
    for pattern in patterns {
        for entry in glob(pattern).expect("Failed to read pattern") {
            match entry {
                Ok(path) => {
                    let mod_path = path.to_str().unwrap().to_string();
                    let mod_path = if let Some(file_prefix) = prefix.clone() {
                        PathBuf::from(file_prefix)
                            .join(PathBuf::from(mod_path))
                            .to_str()
                            .unwrap()
                            .to_string()
                    } else {
                        mod_path
                    };

                    if path.exists() {
                        let content = std::fs::read(path).unwrap();
                        let arc_slice: Arc<[u8]> = Arc::from(content.into_boxed_slice());
                        eszip.add_opaque_data(mod_path.clone(), arc_slice);
                    }

                    static_files.push(mod_path);
                }
                Err(_) => {
                    error!("Error reading pattern {} for static files", pattern)
                }
            };
        }
    }

    if !static_files.is_empty() {
        let file_specifiers_as_bytes = serde_json::to_vec(&static_files).unwrap();
        let arc_slice: Arc<[u8]> = Arc::from(file_specifiers_as_bytes.into_boxed_slice());
        eszip.add_opaque_data(String::from(STATIC_FILES_ESZIP_KEY), arc_slice);
    }
}

fn extract_file_specifiers(eszip: &EszipV2) -> Vec<String> {
    eszip
        .specifiers()
        .iter()
        .filter(|specifier| specifier.starts_with("file:"))
        .cloned()
        .collect()
}

pub struct ExtractEszipPayload {
    pub data: EszipPayloadKind,
    pub folder: PathBuf,
}

fn ensure_unix_relative_path(path: &Path) -> &Path {
    assert!(path.is_relative());
    assert!(!path.to_string_lossy().starts_with('\\'));
    path
}

fn create_module_path(global_specifier: &str, entry_path: &Path, output_folder: &Path) -> PathBuf {
    let cleaned_specifier = global_specifier.replace(entry_path.to_str().unwrap(), "");
    let module_path = PathBuf::from(cleaned_specifier);

    if let Some(parent) = module_path.parent() {
        if parent.parent().is_some() {
            let output_folder_and_mod_folder = output_folder.join(
                parent
                    .strip_prefix("/")
                    .unwrap_or_else(|_| ensure_unix_relative_path(parent)),
            );
            if !output_folder_and_mod_folder.exists() {
                create_dir_all(&output_folder_and_mod_folder).unwrap();
            }
        }
    }

    output_folder.join(
        module_path
            .strip_prefix("/")
            .unwrap_or_else(|_| ensure_unix_relative_path(&module_path)),
    )
}

async fn extract_modules(
    eszip: &EszipV2,
    specifiers: &[String],
    lowest_path: &str,
    output_folder: &Path,
) {
    let main_path = PathBuf::from(lowest_path);
    let entry_path = main_path.parent().unwrap();
    for global_specifier in specifiers {
        let module_path = create_module_path(global_specifier, entry_path, output_folder);
        let module_content = eszip
            .get_module(global_specifier)
            .unwrap()
            .take_source()
            .await
            .unwrap();

        let mut file = File::create(&module_path).unwrap();
        file.write_all(module_content.as_ref()).unwrap();
    }
}

pub async fn extract_eszip(payload: ExtractEszipPayload) {
    let output_folder = payload.folder;
    let mut eszip =
        match eszip_migrate::try_migrate_if_needed(payload_to_eszip(payload.data).await).await {
            Ok(v) => v,
            Err(_old) => {
                warn!("eszip migration failed (give up extract job)");
                return;
            }
        };

    eszip.ensure_read_all().await.unwrap();

    if !output_folder.exists() {
        create_dir_all(&output_folder).unwrap();
    }

    let file_specifiers = extract_file_specifiers(&eszip);
    if let Some(lowest_path) = sb_core::util::path::find_lowest_path(&file_specifiers) {
        extract_modules(&eszip, &file_specifiers, &lowest_path, &output_folder).await;
    } else {
        panic!("Path seems to be invalid");
    }
}

pub async fn extract_from_file(eszip_file: PathBuf, output_path: PathBuf) {
    let eszip_content = fs::read(eszip_file).expect("File does not exist");
    extract_eszip(ExtractEszipPayload {
        data: EszipPayloadKind::VecKind(eszip_content),
        folder: output_path,
    })
    .await;
}

#[cfg(test)]
mod test {
    use crate::{
        extract_eszip, generate_binary_eszip, EmitterFactory, EszipPayloadKind, ExtractEszipPayload,
    };
    use std::fs::remove_dir_all;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[tokio::test]
    #[allow(clippy::arc_with_non_send_sync)]
    async fn test_module_code_no_eszip() {
        let eszip = generate_binary_eszip(
            PathBuf::from("../base/test_cases/npm/index.ts"),
            Arc::new(EmitterFactory::new()),
            None,
            None,
        )
        .await;
        let eszip = eszip.unwrap();
        extract_eszip(ExtractEszipPayload {
            data: EszipPayloadKind::Eszip(eszip),
            folder: PathBuf::from("../base/test_cases/extracted-npm/"),
        })
        .await;

        assert!(PathBuf::from("../base/test_cases/extracted-npm/hello.js").exists());
        remove_dir_all(PathBuf::from("../base/test_cases/extracted-npm/")).unwrap();
    }
}
