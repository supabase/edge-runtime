use crate::emitter::EmitterFactory;
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
use eszip::{EszipV2, ModuleKind, ParseError};
use futures::{AsyncReadExt, AsyncSeekExt};
use glob::glob;
use log::error;
use sb_core::util::sync::AtomicFlag;
use sb_fs::{build_vfs, VfsOpts};
use sb_npm::InnerCliNpmResolverRef;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::fs::{create_dir_all, File};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

mod eszip_parse;

pub mod emitter;
pub mod graph_fs;
pub mod graph_resolver;
pub mod graph_util;
pub mod import_map;
pub mod jsr;
pub mod jsx_util;

pub const VFS_ESZIP_KEY: &str = "---SUPABASE-VFS-DATA-ESZIP---";
pub const SOURCE_CODE_ESZIP_KEY: &str = "---SUPABASE-SOURCE-CODE-ESZIP---";
pub const STATIC_FILES_ESZIP_KEY: &str = "---SUPABASE-STATIC-FILES-ESZIP---";
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

#[derive(Debug, Clone)]
pub enum EszipDataSectionMetadata {
    HasOffset { offset: usize, length: usize },
    PendingOrAlreadyLoaded,
}

#[derive(Debug, Clone)]
pub struct EszipDataSection {
    inner: Arc<Mutex<Cursor<Vec<u8>>>>,
    initial_offset: u64,
    modules: EszipV2Modules,
    partially_read: Arc<AtomicFlag>,
    source_offsets_by_specifier: Arc<Mutex<Option<HashMap<String, EszipDataSectionMetadata>>>>,
}

impl EszipDataSection {
    pub fn new(inner: Cursor<Vec<u8>>, initial_offset: u64, modules: EszipV2Modules) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
            initial_offset,
            modules,
            partially_read: Arc::default(),
            source_offsets_by_specifier: Arc::default(),
        }
    }

    pub async fn read_data_section_by_specifier(
        &self,
        specifier: &str,
    ) -> Result<(), anyhow::Error> {
        if !self.partially_read.is_raised() {
            self.partially_read.raise();
        }

        let mut source_offsets_guard = self.source_offsets_by_specifier.lock().await;
        let source_offsets = source_offsets_guard.get_or_insert_with(|| {
            self.modules
                .0
                .lock()
                .unwrap()
                .iter()
                .filter_map(|(specifier, m)| {
                    if let EszipV2Module::Module { source, .. } = m {
                        match source {
                            EszipV2SourceSlot::Pending { offset, length, .. } => Some((
                                specifier.clone(),
                                EszipDataSectionMetadata::HasOffset {
                                    offset: *offset,
                                    length: *length,
                                },
                            )),

                            EszipV2SourceSlot::Ready(_) | EszipV2SourceSlot::Taken => Some((
                                specifier.clone(),
                                EszipDataSectionMetadata::PendingOrAlreadyLoaded,
                            )),
                        }
                    } else {
                        None
                    }
                })
                .collect::<HashMap<_, _>>()
        });

        let Some(metadata) = source_offsets.get_mut(specifier) else {
            bail!("given specifier does not exist in the eszip header")
        };

        let (offset, length) = match metadata {
            metadata @ &mut EszipDataSectionMetadata::HasOffset { offset, length } => {
                *metadata = EszipDataSectionMetadata::PendingOrAlreadyLoaded;
                (offset, length)
            }

            _ => return Ok(()),
        };

        drop(source_offsets_guard);

        let mut inner = self.inner.lock().await;
        let mut io = AllowStdIo::new({
            // NOTE: 4 byte offset in the middle represents the full source length.
            inner.set_position(self.initial_offset + 4 + offset as u64);
            inner.by_ref()
        });

        let mut source_bytes = vec![0u8; length];
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

        let wakers = {
            let mut modules = self.modules.0.lock().unwrap();
            let module = modules.get_mut(specifier).expect("module not found");
            match module {
                EszipV2Module::Module { ref mut source, .. } => {
                    let slot = std::mem::replace(
                        source,
                        EszipV2SourceSlot::Ready(Arc::from(source_bytes)),
                    );

                    match slot {
                        EszipV2SourceSlot::Pending { wakers, .. } => wakers,
                        _ => panic!("already populated source slot"),
                    }
                }
                _ => panic!("invalid module type"),
            }
        };
        for w in wakers {
            w.wake();
        }

        Ok(())
    }

    pub async fn read_data_section_all(self: Arc<Self>) -> Result<(), ParseError> {
        if self.partially_read.is_raised() {
            panic!("can't invoke if part of the data section has been already read")
        }

        // NOTE: Below codes is roughly originated from eszip@0.60.0/src/v2.rs

        let this = Arc::into_inner(self).unwrap();
        let modules = this.modules;

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
                    Some((*offset, (*length, specifier.clone())))
                } else {
                    None
                }
            })
            .collect::<HashMap<_, _>>();

        while read < sources_len {
            let (length, specifier) = source_offsets
                .remove(&read)
                .ok_or(ParseError::InvalidV2SourceOffset(read))?;

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

            let wakers = {
                let mut modules = modules.0.lock().unwrap();
                let module = modules.get_mut(&specifier).expect("module not found");
                match module {
                    EszipV2Module::Module { ref mut source, .. } => {
                        let slot = std::mem::replace(
                            source,
                            EszipV2SourceSlot::Ready(Arc::from(source_bytes)),
                        );

                        match slot {
                            EszipV2SourceSlot::Pending { wakers, .. } => wakers,
                            _ => panic!("already populated source slot"),
                        }
                    }
                    _ => panic!("invalid module type"),
                }
            };

            for w in wakers {
                w.wake();
            }
        }

        Ok(())
    }
}

pub async fn payload_to_eszip(
    eszip_payload_kind: EszipPayloadKind,
) -> (EszipV2, Option<Arc<EszipDataSection>>) {
    match eszip_payload_kind {
        EszipPayloadKind::Eszip(data) => (data, None),
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

            (eszip, Some(Arc::new(data_section)))
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
    let eszip = create_eszip_from_graph_raw(graph, Some(emitter_factory.clone())).await;

    if let Ok(mut eszip) = eszip {
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
                let snapshot =
                    managed.serialized_valid_snapshot_for_system(&NpmSystemInfo::default());
                if !snapshot.as_serialized().packages.is_empty() {
                    let (root_dir, files) = build_vfs(VfsOpts {
                        npm_resolver: resolver.clone(),
                        npm_registry_api: emitter_factory.npm_api().await.clone(),
                        npm_cache: emitter_factory.npm_cache().await.clone(),
                        npm_resolution: emitter_factory.npm_resolution().await.clone(),
                    })?
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

        let npm_vfs = serde_json::to_vec(&npm_vfs).unwrap().to_vec();
        let boxed_slice = npm_vfs.into_boxed_slice();

        eszip.add_opaque_data(String::from(VFS_ESZIP_KEY), Arc::from(boxed_slice));
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
    } else {
        eszip
    }
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
    let (eszip, maybe_data_section) = payload_to_eszip(payload.data).await;
    let output_folder = payload.folder;

    if let Some(section) = maybe_data_section {
        section.read_data_section_all().await.unwrap();
    }

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
