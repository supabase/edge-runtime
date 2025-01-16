use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Cursor;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::bail;
use anyhow::Context;
use deno::deno_ast;
use deno::deno_fs::FileSystem;
use deno::deno_fs::RealFs;
use deno::deno_npm::NpmSystemInfo;
use deno::npm::InnerCliNpmResolverRef;
use deno_core::url::Url;
use deno_core::FastString;
use deno_core::JsBuffer;
use deno_core::ModuleSpecifier;
use error::EszipError;
use eszip::v2::EszipV2Module;
use eszip::v2::EszipV2Modules;
use eszip::v2::EszipV2SourceSlot;
use eszip::EszipV2;
use eszip::Module;
use eszip::ModuleKind;
use eszip::ParseError;
use eszip_async_trait::AsyncEszipDataRead;
use eszip_async_trait::NPM_RC_SCOPES_KEY;
use eszip_async_trait::SOURCE_CODE_ESZIP_KEY;
use eszip_async_trait::STATIC_FILES_ESZIP_KEY;
use eszip_async_trait::SUPABASE_ESZIP_VERSION;
use eszip_async_trait::SUPABASE_ESZIP_VERSION_KEY;
use eszip_async_trait::VFS_ESZIP_KEY;
use fs::build_vfs;
use fs::VfsOpts;
use futures::future::OptionFuture;
use futures::io::AllowStdIo;
use futures::io::BufReader;
use futures::AsyncReadExt;
use futures::AsyncSeekExt;
use glob::glob;
use scopeguard::ScopeGuard;
use tokio::fs::create_dir_all;
use tokio::sync::Mutex;

use crate::emitter::EmitterFactory;
use crate::extract_modules;
use crate::graph::create_eszip_from_graph_raw;
use crate::graph::create_graph;

mod parse;

pub mod error;
pub mod migrate;

#[derive(Debug)]
pub enum EszipPayloadKind {
  JsBufferKind(JsBuffer),
  VecKind(Vec<u8>),
  Eszip(EszipV2),
}

async fn read_u32<R: futures::io::AsyncRead + Unpin>(
  reader: &mut R,
) -> Result<u32, ParseError> {
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
        options: self.eszip.options,
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
  fn new(
    eszip: EszipV2,
    maybe_data_section: Option<Arc<EszipDataSection>>,
  ) -> Self {
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
            log::error!(
              "failed to read module data from the data section: {}",
              err
            );
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
      self
        .ensure_module(SUPABASE_ESZIP_VERSION_KEY)
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
  modules: EszipV2Modules,
  options: eszip::v2::Options,
  initial_offset: u64,
  sources_len: Arc<Mutex<Option<u64>>>,
  locs_by_specifier:
    Arc<Mutex<Option<HashMap<String, EszipDataSectionMetadata>>>>,
  loaded_locs_by_specifier: Arc<Mutex<HashMap<String, EszipDataLoc>>>,
}

impl EszipDataSection {
  pub fn new(
    inner: Cursor<Vec<u8>>,
    initial_offset: u64,
    modules: EszipV2Modules,
    options: eszip::v2::Options,
  ) -> Self {
    Self {
      inner: Arc::new(Mutex::new(inner)),
      modules,
      options,
      initial_offset,
      sources_len: Arc::default(),
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
      self
        .modules
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
              loc.source_length = 0;
              loc.source_offset = 0;
            }
          }

          if let EszipV2SourceSlot::Pending { offset, length, .. } =
            source_map_slot
          {
            loc.source_map_offset = *offset;
            loc.source_map_length = *length;
          } else if loc.source_length == 0 && loc.source_offset == 0 {
            return Some((
              specifier.clone(),
              EszipDataSectionMetadata::PendingOrAlreadyLoaded,
            ));
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
      &mut EszipDataSectionMetadata::HasLocation(loc) => {
        self
          .loaded_locs_by_specifier
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

    let source_bytes = 'scope: {
      if loc.source_length == 0 {
        break 'scope None::<Vec<u8>>;
      }

      let wake_guard = scopeguard::guard(&self.modules, |modules| {
        Self::wake_source_slot(modules, specifier, || EszipV2SourceSlot::Taken);
      });

      let source_bytes = eszip::v2::Section::read_with_size(
        &mut io,
        self.options,
        loc.source_length,
      )
      .await?;

      if !source_bytes.is_checksum_valid() {
        return Err(ParseError::InvalidV2SourceHash(specifier.to_string()))
          .context("invalid source hash");
      }

      let _ = ScopeGuard::into_inner(wake_guard);

      Some(source_bytes.into_content())
    };

    if let Some(bytes) = source_bytes {
      Self::wake_source_slot(&self.modules, specifier, move || {
        EszipV2SourceSlot::Ready(Arc::from(bytes))
      });
    }

    let source_map_bytes = 'scope: {
      if loc.source_map_length == 0 {
        break 'scope None::<Vec<u8>>;
      }

      let sources_len = {
        let mut guard = self.sources_len.lock().await;

        match &mut *guard {
          Some(len) => *len,
          opt @ None => {
            let mut io = AllowStdIo::new({
              inner.set_position(self.initial_offset);
              inner.by_ref()
            });

            let sources_len = read_u32(&mut io).await? as usize;

            *opt = Some(sources_len as u64);
            sources_len as u64
          }
        }
      };

      let mut io = AllowStdIo::new({
        // NOTE: 4 byte offset in the middle represents the full source / source map length.
        inner.set_position(
          self.initial_offset
            + 4
            + sources_len
            + 4
            + loc.source_map_offset as u64,
        );
        inner.by_ref()
      });

      let wake_guard = scopeguard::guard(&self.modules, |modules| {
        Self::wake_source_map_slot(modules, specifier, || {
          EszipV2SourceSlot::Taken
        });
      });

      let source_map_bytes = eszip::v2::Section::read_with_size(
        &mut io,
        self.options,
        loc.source_map_length,
      )
      .await?;

      if !source_map_bytes.is_checksum_valid() {
        return Err(ParseError::InvalidV2SourceHash(specifier.to_string()))
          .context("invalid source hash");
      }

      let _ = ScopeGuard::into_inner(wake_guard);

      Some(source_map_bytes.into_content())
    };

    if let Some(bytes) = source_map_bytes {
      Self::wake_source_map_slot(&self.modules, specifier, move || {
        EszipV2SourceSlot::Ready(Arc::from(bytes))
      });
    }

    Ok(())
  }

  pub async fn read_data_section_all(
    self: Arc<Self>,
  ) -> Result<(), ParseError> {
    // NOTE: Below codes is roughly originated from eszip@0.72.2/src/v2.rs

    let this = Arc::into_inner(self).unwrap();
    let modules = this.modules;
    let checksum_size = this
      .options
      .checksum_size()
      .expect("checksum size must be known") as usize;

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
        read += length + checksum_size;

        io.seek(SeekFrom::Current((length + checksum_size) as i64))
          .await
          .unwrap();

        continue;
      }

      let source_bytes =
        eszip::v2::Section::read_with_size(&mut io, this.options, length)
          .await?;

      if !source_bytes.is_checksum_valid() {
        return Err(ParseError::InvalidV2SourceHash(specifier));
      }

      read += source_bytes.total_len();

      Self::wake_source_slot(&modules, &specifier, move || {
        EszipV2SourceSlot::Ready(Arc::from(source_bytes.into_content()))
      });
    }

    let sources_maps_len = read_u32(&mut io).await? as usize;
    let mut read = 0;

    while read < sources_maps_len {
      let (length, specifier, need_load) = source_map_offsets
        .remove(&read)
        .ok_or(ParseError::InvalidV2SourceOffset(read))?;

      if !need_load {
        read += length + checksum_size;

        io.seek(SeekFrom::Current((length + checksum_size) as i64))
          .await
          .unwrap();

        continue;
      }

      let source_map_bytes =
        eszip::v2::Section::read_with_size(&mut io, this.options, length)
          .await?;

      if !source_map_bytes.is_checksum_valid() {
        return Err(ParseError::InvalidV2SourceHash(specifier));
      }

      read += source_map_bytes.total_len();

      Self::wake_source_map_slot(&modules, &specifier, move || {
        EszipV2SourceSlot::Ready(Arc::from(source_map_bytes.into_content()))
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

  fn wake_source_slot<F>(
    modules: &EszipV2Modules,
    specifier: &str,
    new_slot_fn: F,
  ) where
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

  fn wake_source_map_slot<F>(
    modules: &EszipV2Modules,
    specifier: &str,
    new_slot_fn: F,
  ) where
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

pub async fn payload_to_eszip(
  eszip_payload_kind: EszipPayloadKind,
) -> Result<LazyLoadableEszip, anyhow::Error> {
  match eszip_payload_kind {
    EszipPayloadKind::Eszip(eszip) => Ok(LazyLoadableEszip::new(eszip, None)),
    _ => {
      let bytes = match eszip_payload_kind {
        EszipPayloadKind::JsBufferKind(js_buffer) => Vec::from(&*js_buffer),
        EszipPayloadKind::VecKind(vec) => vec,
        _ => unreachable!(),
      };

      let mut io = AllowStdIo::new(Cursor::new(bytes));
      let mut bufreader = BufReader::new(&mut io);

      let eszip = parse::parse_v2_header(&mut bufreader).await?;

      let initial_offset = bufreader.stream_position().await.unwrap();
      let data_section = EszipDataSection::new(
        io.into_inner(),
        initial_offset,
        eszip.modules.clone(),
        eszip.options,
      );

      Ok(LazyLoadableEszip::new(eszip, Some(Arc::new(data_section))))
    }
  }
}

pub async fn generate_binary_eszip<P>(
  file: P,
  emitter_factory: Arc<EmitterFactory>,
  maybe_module_code: Option<FastString>,
  maybe_import_map_url: Option<String>,
  maybe_checksum: Option<eszip::v2::Checksum>,
) -> Result<EszipV2, anyhow::Error>
where
  P: AsRef<Path>,
{
  let file = file.as_ref();
  let cjs_tracker = emitter_factory.cjs_tracker()?.clone();
  let graph = create_graph(
    file.to_path_buf(),
    emitter_factory.clone(),
    &maybe_module_code,
  )
  .await?;

  let specifier = ModuleSpecifier::parse(
    &Url::from_file_path(file)
      .map(|it| Cow::Owned(it.to_string()))
      .ok()
      .unwrap_or("http://localhost".into()),
  )
  .unwrap();

  let m = graph
    .get(&specifier)
    .context("cannot get a module")?
    .js()
    .context("not a js module")?;

  let media_type = m.media_type;
  let is_cjs = cjs_tracker.is_cjs_with_known_is_script(
    &specifier,
    m.media_type,
    m.is_script,
  )?;

  let mut eszip =
    create_eszip_from_graph_raw(graph, Some(emitter_factory.clone())).await?;
  if let Some(checksum) = maybe_checksum {
    eszip.set_checksum(checksum);
  }

  let source_code: Arc<str> = if let Some(code) = maybe_module_code {
    code.as_str().into()
  } else {
    String::from_utf8(RealFs.read_file_sync(file, None)?.to_vec())?.into()
  };

  let emit_source = emitter_factory
    .emitter()
    .unwrap()
    .emit_parsed_source(
      &specifier,
      media_type,
      deno_ast::ModuleKind::from_is_cjs(is_cjs),
      &source_code,
    )
    .await?;

  let bin_code: Arc<[u8]> = emit_source.as_bytes().into();
  let resolver = emitter_factory.npm_resolver().await.cloned()?;

  let (npm_vfs, _npm_files) = match resolver.clone().as_inner() {
    InnerCliNpmResolverRef::Managed(managed) => {
      let snapshot =
        managed.serialized_valid_snapshot_for_system(&NpmSystemInfo::default());
      if !snapshot.as_serialized().packages.is_empty() {
        let mut count = 0;
        let (root_dir, files) = build_vfs(
          VfsOpts {
            npm_resolver: resolver.clone(),
          },
          |_path, _key, content| {
            let key = format!("vfs://{}", count);

            count += 1;
            eszip.add_opaque_data(key.clone(), content.into());
            key
          },
        )?
        .into_dir_and_files();

        let snapshot = managed
          .serialized_valid_snapshot_for_system(&NpmSystemInfo::default());

        eszip.add_npm_snapshot(snapshot);

        (Some(root_dir), files)
      } else {
        (None, Vec::new())
      }
    }
    InnerCliNpmResolverRef::Byonm(_) => unreachable!(),
  };

  let npm_vfs = rkyv::to_bytes::<_, 1024>(&npm_vfs)
    .with_context(|| "cannot serialize vfs data")?;

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

  let resolved_npm_rc = emitter_factory.resolved_npm_rc().await?;
  let modified_scopes = resolved_npm_rc
    .scopes
    .iter()
    .filter_map(|(k, v)| {
      Some((k.clone(), {
        let mut url = v.registry_url.clone();

        if url.scheme() != "http" && url.scheme() != "https" {
          return None;
        }
        if url.port().is_none() && url.path() == "/" {
          return None;
        }
        if url.set_port(None).is_err() {
          return None;
        }
        if url.set_host(Some("localhost")).is_err() {
          return None;
        }
        if url.set_scheme("https").is_err() {
          return None;
        }

        url.to_string()
      }))
    })
    .collect::<HashMap<_, _>>();

  eszip.add_opaque_data(
    String::from(NPM_RC_SCOPES_KEY),
    Arc::from(
      rkyv::to_bytes::<_, 1024>(&modified_scopes)
        .with_context(|| "cannot serialize vfs data")?
        .into_boxed_slice(),
    ),
  );

  Ok(eszip)
}

pub async fn include_glob_patterns_in_eszip<P>(
  patterns: Vec<&str>,
  eszip: &mut EszipV2,
  base_dir: P,
) -> Result<(), anyhow::Error>
where
  P: AsRef<Path>,
{
  let cwd = std::env::current_dir();
  let base_dir = base_dir.as_ref();
  let mut specifiers: Vec<String> = vec![];

  for pattern in patterns {
    for entry in glob(pattern).expect("Failed to read pattern") {
      match entry {
        Ok(path) => {
          let path = cwd.as_ref().unwrap().join(path);
          let (path, rel) = match pathdiff::diff_paths(&path, base_dir) {
            Some(rel) => (path, rel.to_string_lossy().to_string()),
            None => (path.clone(), path.to_string_lossy().to_string()),
          };

          if path.exists() {
            let specifier = format!("static:{}", rel.as_str());

            eszip.add_opaque_data(
              specifier.clone(),
              Arc::from(std::fs::read(path).unwrap().into_boxed_slice()),
            );

            specifiers.push(specifier);
          }
        }

        Err(_) => {
          log::error!("Error reading pattern {} for static files", pattern)
        }
      };
    }
  }

  if !specifiers.is_empty() {
    eszip.add_opaque_data(
      String::from(STATIC_FILES_ESZIP_KEY),
      Arc::from(
        rkyv::to_bytes::<_, 1024>(&specifiers)
          .with_context(|| {
            "cannot serialize accessible paths for static files"
          })?
          .into_boxed_slice(),
      ),
    );
  }

  Ok(())
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

pub async fn extract_eszip(payload: ExtractEszipPayload) -> bool {
  let output_folder = payload.folder;
  let eszip = match payload_to_eszip(payload.data).await {
    Ok(v) => v,
    Err(err) => {
      log::error!("{err:?}");
      return false;
    }
  };

  let mut eszip = match migrate::try_migrate_if_needed(eszip).await {
    Ok(v) => v,
    Err(_old) => {
      log::error!("eszip migration failed (give up extract job)");
      return false;
    }
  };

  eszip.ensure_read_all().await.unwrap();

  if !output_folder.exists() {
    create_dir_all(&output_folder).await.unwrap();
  }

  let file_specifiers = extract_file_specifiers(&eszip);
  if let Some(lowest_path) =
    deno::util::path::find_lowest_path(&file_specifiers)
  {
    extract_modules(&eszip, &file_specifiers, &lowest_path, &output_folder)
      .await;
    true
  } else {
    panic!("Path seems to be invalid");
  }
}
