use crate::emitter::EmitterFactory;
use crate::graph_util::{create_eszip_from_graph_raw, create_graph};
use deno_core::error::AnyError;
use deno_core::{serde_json, JsBuffer};
use deno_fs::{FileSystem, RealFs};
use deno_npm::NpmSystemInfo;
use eszip::EszipV2;
use sb_fs::{build_vfs, VfsOpts};
use std::path::PathBuf;
use std::sync::Arc;

pub mod emitter;
pub mod graph_resolver;
pub mod graph_util;

pub const VFS_ESZIP_KEY: &str = "---SUPABASE-VFS-DATA-ESZIP---";
pub const SOURCE_CODE_ESZIP_KEY: &str = "---SUPABASE-SOURCE-CODE-ESZIP---";

#[derive(Debug)]
pub enum EszipPayloadKind {
    JsBufferKind(JsBuffer),
    VecKind(Vec<u8>),
    Eszip(EszipV2),
}

pub async fn generate_binary_eszip(
    file: PathBuf,
    emitter_factory: Arc<EmitterFactory>,
) -> Result<EszipV2, AnyError> {
    let graph = create_graph(file.clone(), emitter_factory.clone()).await;
    let eszip = create_eszip_from_graph_raw(graph, Some(emitter_factory.clone())).await;

    if let Ok(mut eszip) = eszip {
        let entry_content = RealFs.read_file_sync(file.clone().as_path()).unwrap();
        let npm_res = emitter_factory.npm_resolution();

        let (npm_vfs, _npm_files) = if npm_res.has_packages() {
            let (root_dir, files) = build_vfs(VfsOpts {
                npm_resolver: emitter_factory.npm_resolver().clone(),
                npm_registry_api: emitter_factory.npm_api().clone(),
                npm_cache: emitter_factory.npm_cache().clone(),
                npm_resolution: emitter_factory.npm_resolution().clone(),
            })?
            .into_dir_and_files();

            let snapshot = npm_res.serialized_valid_snapshot_for_system(&NpmSystemInfo::default());
            eszip.add_npm_snapshot(snapshot);
            (Some(root_dir), files)
        } else {
            (None, Vec::new())
        };

        let npm_vfs = serde_json::to_string(&npm_vfs)?.as_bytes().to_vec();
        let boxed_slice = npm_vfs.into_boxed_slice();

        eszip.add_opaque_data(String::from(VFS_ESZIP_KEY), Arc::from(boxed_slice));
        eszip.add_opaque_data(
            String::from(SOURCE_CODE_ESZIP_KEY),
            Arc::from(entry_content.into_boxed_slice()),
        );

        Ok(eszip)
    } else {
        eszip
    }
}
