use log::{error, warn};

use crate::{errors::EszipError, LazyLoadableEszip};

pub async fn try_migrate_if_needed(
    mut eszip: LazyLoadableEszip,
) -> Result<LazyLoadableEszip, LazyLoadableEszip> {
    if let Err(err) = eszip.ensure_version().await {
        match err.downcast_ref::<EszipError>() {
            Some(err) => {
                warn!("{}: will attempt migration", err);

                macro_rules! cont {
                    ($name: ident, $label:tt, $expr:expr) => {
                        #[allow(unused_mut)]
                        let mut $name = {
                            match $expr {
                                Ok(v) => v,
                                err @ Err(_) => break $label err,
                            }
                        };
                    };
                }

                let result = match err {
                    EszipError::UnsupportedVersion { expected, found } => {
                        match (expected, found.as_deref()) {
                            (&b"1.1", None) => 'scope: {
                                cont!(v1, 'scope, v0::try_migrate_v0_v1(&mut eszip).await);
                                v1_1::try_migrate_v1_v1_1(&mut v1).await
                            }
                            (&b"1.1", Some(b"1")) => v1_1::try_migrate_v1_v1_1(&mut eszip).await,
                            _ => unreachable!(),
                        }
                    }
                };

                match result {
                    Ok(migrated) => Ok(migrated),
                    Err(err) => {
                        error!("{:?}", err);
                        Err(eszip)
                    }
                }
            }

            None => {
                error!("failed to migrate (found unexpected error)");
                Err(eszip)
            }
        }
    } else {
        Ok(eszip)
    }
}

mod v0 {
    use std::{collections::HashSet, sync::Arc};

    use anyhow::Context;
    use deno_core::serde_json;
    use eszip::{v2::EszipV2Modules, EszipV2};
    use eszip_async_trait::{AsyncEszipDataRead, SUPABASE_ESZIP_VERSION_KEY, VFS_ESZIP_KEY};
    use futures::future::OptionFuture;
    use once_cell::sync::Lazy;
    use serde::{Deserialize, Serialize};

    use crate::{eszip_migrate::v1, LazyLoadableEszip};

    #[derive(Serialize, Deserialize, Debug)]
    pub struct Directory {
        pub name: String,
        pub entries: Vec<Entry>,
    }

    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct File {
        pub name: String,
        pub offset: u64,
        pub len: u64,
        pub content: Option<Vec<u8>>,
    }

    #[derive(Serialize, Deserialize, Debug)]
    pub struct Symlink {
        pub name: String,
        pub dest_parts: Vec<String>,
    }

    #[derive(Serialize, Deserialize, Debug)]
    pub enum Entry {
        Dir(Directory),
        File(File),
        Symlink(Symlink),
    }

    pub async fn try_migrate_v0_v1(
        v0_eszip: &mut LazyLoadableEszip,
    ) -> Result<LazyLoadableEszip, anyhow::Error> {
        let mut v1_eszip = LazyLoadableEszip::new(
            EszipV2 {
                modules: EszipV2Modules::default(),
                npm_snapshot: v0_eszip.npm_snapshot.take(),
                options: v0_eszip.options,
            },
            None,
        );

        v0_eszip
            .ensure_read_all()
            .await
            .with_context(|| "failed to load v0 eszip data")?;

        let vfs_mod_data = OptionFuture::<_>::from(
            v0_eszip
                .ensure_module(VFS_ESZIP_KEY)
                .map(|it| async move { it.source().await }),
        )
        .await
        .flatten();

        //STATIC_FILES_ESZIP_KEY

        let v1_dir = if let Some(data) = vfs_mod_data {
            let mut count = 0;
            let v0_dir = serde_json::from_slice::<Option<Directory>>(data.as_ref())
                .with_context(|| "failed to parse v0 structure")?;

            fn migrate_dir_v0_v1(
                v0_dir: self::Directory,
                v1_eszip: &mut LazyLoadableEszip,
                count: &mut i32,
            ) -> v1::Directory {
                let mut v1_dir = v1::Directory {
                    name: v0_dir.name.clone(),
                    entries: vec![],
                };

                let v1_dir_entries = &mut v1_dir.entries;

                for entry in v0_dir.entries.into_iter() {
                    match entry {
                        Entry::Dir(v0_sub_dir) => {
                            v1_dir_entries.push(v1::Entry::Dir(migrate_dir_v0_v1(
                                v0_sub_dir, v1_eszip, count,
                            )));
                        }

                        Entry::File(v0_sub_file) => {
                            let key = format!("vfs://{}", *count);
                            let data = v0_sub_file.content;

                            *count += 1;
                            v1_dir_entries.push(v1::Entry::File(v1::File {
                                key: key.clone(),
                                name: v0_sub_file.name,
                                offset: v0_sub_file.offset,
                                len: v0_sub_file.len,
                            }));

                            if let Some(data) = data {
                                v1_eszip.add_opaque_data(key, data.into());
                            }
                        }

                        Entry::Symlink(v0_sub_symlink) => {
                            v1_dir_entries.push(v1::Entry::Symlink(v1::Symlink {
                                name: v0_sub_symlink.name,
                                dest_parts: v0_sub_symlink.dest_parts,
                            }));
                        }
                    }
                }

                v1_dir
            }

            v0_dir.map(|it| migrate_dir_v0_v1(it, &mut v1_eszip, &mut count))
        } else {
            None
        };

        let v1_vfs_data = rkyv::to_bytes::<_, 1024>(&v1_dir)
            .with_context(|| "failed to serialize v1 vfs data")?;

        v1_eszip.add_opaque_data(
            String::from(SUPABASE_ESZIP_VERSION_KEY),
            Arc::from(b"1" as &[u8]),
        );

        v1_eszip.add_opaque_data(
            String::from(VFS_ESZIP_KEY),
            Arc::from(v1_vfs_data.into_boxed_slice()),
        );

        static BLOCKLIST: Lazy<HashSet<&str>> =
            Lazy::new(|| HashSet::from([SUPABASE_ESZIP_VERSION_KEY, VFS_ESZIP_KEY]));

        let specifiers = v0_eszip.specifiers();
        let mut v0_modules = v0_eszip.modules.0.lock().unwrap();
        let mut v1_modules = v1_eszip.modules.0.lock().unwrap();

        for specifier in specifiers {
            if BLOCKLIST.contains(specifier.as_str()) {
                continue;
            }

            let module = v0_modules.remove(&specifier).unwrap();

            v1_modules.insert(specifier, module);
        }

        drop(v1_modules);

        Ok(v1_eszip)
    }
}

mod v1 {
    use rkyv::{Archive, Deserialize, Serialize};

    #[derive(Archive, Serialize, Deserialize, Debug)]
    #[archive(
        check_bytes,
        bound(serialize = "__S: rkyv::ser::ScratchSpace + rkyv::ser::Serializer")
    )]
    #[archive_attr(check_bytes(
        bound = "__C: rkyv::validation::ArchiveContext, <__C as rkyv::Fallible>::Error: std::error::Error"
    ))]
    pub struct Directory {
        pub name: String,
        // should be sorted by name
        #[omit_bounds]
        #[archive_attr(omit_bounds)]
        pub entries: Vec<Entry>,
    }

    #[derive(Archive, Serialize, Deserialize, Debug, Clone)]
    #[archive(check_bytes)]
    pub struct File {
        pub key: String,
        pub name: String,
        pub offset: u64,
        pub len: u64,
    }

    #[derive(Archive, Serialize, Deserialize, Debug)]
    #[archive(check_bytes)]
    pub struct Symlink {
        pub name: String,
        pub dest_parts: Vec<String>,
    }

    #[derive(Archive, Serialize, Deserialize, Debug)]
    #[cfg_attr(test, derive(enum_as_inner::EnumAsInner))]
    #[archive(check_bytes)]
    pub enum Entry {
        Dir(Directory),
        File(File),
        Symlink(Symlink),
    }
}

mod v1_1 {
    use std::sync::Arc;

    use anyhow::{bail, Context};
    use eszip::{v2::Checksum, EszipV2};
    use eszip_async_trait::{AsyncEszipDataRead, SUPABASE_ESZIP_VERSION_KEY, VFS_ESZIP_KEY};
    use futures::future::OptionFuture;

    use crate::LazyLoadableEszip;

    use super::v1;

    pub async fn try_migrate_v1_v1_1(
        v1_eszip: &mut LazyLoadableEszip,
    ) -> Result<LazyLoadableEszip, anyhow::Error> {
        let mut v1_1_eszip = LazyLoadableEszip::new(
            EszipV2 {
                modules: v1_eszip.modules.clone(),
                npm_snapshot: v1_eszip.npm_snapshot.take(),
                options: v1_eszip.options,
            },
            None,
        );

        v1_eszip
            .ensure_read_all()
            .await
            .with_context(|| "failed to load v1 eszip data")?;

        v1_1_eszip.set_checksum(Checksum::NoChecksum);
        v1_1_eszip.add_opaque_data(
            String::from(SUPABASE_ESZIP_VERSION_KEY),
            Arc::from(b"1.1" as &[u8]),
        );

        let v1_dir = {
            let Some(vfs_mod_data) = OptionFuture::<_>::from(
                v1_eszip
                    .ensure_module(VFS_ESZIP_KEY)
                    .map(|it| async move { it.take_source().await }),
            )
            .await
            .flatten() else {
                return Ok(v1_1_eszip);
            };

            let Ok(v1_dir) = rkyv::from_bytes::<Option<v1::Directory>>(&vfs_mod_data) else {
                bail!("cannot deserialize vfs data");
            };

            v1_dir
        };

        let v1_1_dir = if let Some(mut v1_dir) = v1_dir {
            if v1_dir.name != "node_modules" {
                bail!("malformed vfs data (expected node_modules)");
            }

            v1_dir.name = "localhost".into();

            Some(v1::Directory {
                name: "node_modules".into(),
                entries: vec![v1::Entry::Dir(v1_dir)],
            })
        } else {
            None
        };

        let v1_1_vfs_data = rkyv::to_bytes::<_, 1024>(&v1_1_dir)
            .with_context(|| "failed to serialize v1.1 vfs data")?;

        v1_1_eszip.add_opaque_data(
            String::from(VFS_ESZIP_KEY),
            Arc::from(v1_1_vfs_data.into_boxed_slice()),
        );

        Ok(v1_1_eszip)
    }
}

#[cfg(test)]
mod test {
    use eszip_async_trait::{AsyncEszipDataRead, VFS_ESZIP_KEY};
    use std::path::PathBuf;
    use tokio::fs;

    use crate::{
        eszip_migrate::try_migrate_if_needed, extract_eszip, payload_to_eszip, EszipPayloadKind,
        ExtractEszipPayload,
    };

    const MIGRATE_TEST_DIR: &str = "../base/test_cases/eszip-migration";

    async fn test_extract_eszip(orig: PathBuf, target: PathBuf) {
        let tmp_dir = tempfile::tempdir().unwrap();
        let (_orig_buf, target_buf) = {
            (
                fs::read(orig).await.unwrap(),
                fs::read(target).await.unwrap(),
            )
        };

        let payload = ExtractEszipPayload {
            data: EszipPayloadKind::VecKind(target_buf),
            folder: tmp_dir.path().to_path_buf(),
        };

        assert!(extract_eszip(payload).await);

        // TODO(Nyannyacha): It seems to be returning a buffer for the transpiled source rather than
        // the original source. Fix that issue and uncomment below.

        // let tmp_file_buf = fs::read(tmp_dir.path().join("index.ts")).await.unwrap();
        // assert_eq!(orig_buf, tmp_file_buf);
    }

    #[tokio::test]
    async fn test_extract_v0() {
        test_extract_eszip(
            PathBuf::from(format!("{}/npm-supabase-js/index.ts", MIGRATE_TEST_DIR)),
            PathBuf::from(format!("{}/npm-supabase-js/v0.eszip", MIGRATE_TEST_DIR)),
        )
        .await;
    }

    #[tokio::test]
    async fn test_extract_v1() {
        test_extract_eszip(
            PathBuf::from(format!("{}/npm-supabase-js/index.ts", MIGRATE_TEST_DIR)),
            PathBuf::from(format!("{}/npm-supabase-js/v1.eszip", MIGRATE_TEST_DIR)),
        )
        .await;
    }

    #[tokio::test]
    #[should_panic]
    async fn test_extract_v0_corrupted() {
        test_extract_eszip(
            PathBuf::from(format!("{}/npm-supabase-js/index.ts", MIGRATE_TEST_DIR)),
            PathBuf::from(format!(
                "{}/npm-supabase-js/v0_corrupted.eszip",
                MIGRATE_TEST_DIR
            )),
        )
        .await;
    }

    #[tokio::test]
    #[should_panic]
    async fn test_extract_v1_corrupted() {
        test_extract_eszip(
            PathBuf::from(format!("{}/npm-supabase-js/index.ts", MIGRATE_TEST_DIR)),
            PathBuf::from(format!(
                "{}/npm-supabase-js/v1_corrupted.eszip",
                MIGRATE_TEST_DIR
            )),
        )
        .await;
    }

    async fn test_vfs_npm_registry_migration_1_45_x(buf: Vec<u8>) {
        let eszip = payload_to_eszip(EszipPayloadKind::VecKind(buf))
            .await
            .unwrap();
        let migrated = try_migrate_if_needed(eszip).await.unwrap();

        let vfs_data = migrated
            .ensure_module(VFS_ESZIP_KEY)
            .unwrap()
            .source()
            .await
            .unwrap();

        let dir = rkyv::from_bytes::<Option<super::v1::Directory>>(&vfs_data)
            .unwrap()
            .unwrap();

        assert_eq!(dir.name, "node_modules");

        let first_child = dir.entries.first().unwrap().as_dir().unwrap();

        assert_eq!(first_child.name, "localhost");
    }

    #[tokio::test]
    async fn test_vfs_registry_migration_v0() {
        test_vfs_npm_registry_migration_1_45_x(
            fs::read(PathBuf::from(format!(
                "{}/npm-supabase-js/v0.eszip",
                MIGRATE_TEST_DIR
            )))
            .await
            .unwrap(),
        )
        .await;
    }

    #[tokio::test]
    async fn test_vfs_registry_migration_v1() {
        test_vfs_npm_registry_migration_1_45_x(
            fs::read(PathBuf::from(format!(
                "{}/npm-supabase-js/v1.eszip",
                MIGRATE_TEST_DIR
            )))
            .await
            .unwrap(),
        )
        .await;
    }

    #[tokio::test]
    async fn test_vfs_registry_migration_v1_1_xx_hash3() {
        test_vfs_npm_registry_migration_1_45_x(
            fs::read(PathBuf::from(format!(
                "{}/npm-supabase-js/v1_1_xx_hash3.eszip",
                MIGRATE_TEST_DIR
            )))
            .await
            .unwrap(),
        )
        .await;
    }

    #[tokio::test]
    async fn test_vfs_registry_migration_v1_1_no_checksum() {
        test_vfs_npm_registry_migration_1_45_x(
            fs::read(PathBuf::from(format!(
                "{}/npm-supabase-js/v1_1_no_checksum.eszip",
                MIGRATE_TEST_DIR
            )))
            .await
            .unwrap(),
        )
        .await;
    }
}
