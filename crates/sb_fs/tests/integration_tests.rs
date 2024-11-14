use std::{collections::HashMap, time::Duration};

use anyhow::Context;
use base::{server::ServerFlags, utils::test_utils::TestBedBuilder};
use ctor::ctor;
use deno_core::serde_json;
use event_worker::events::{LogLevel, WorkerEvents};
use hyper_v014::{body::to_bytes, Body, StatusCode};
use rand::RngCore;
use serde::Deserialize;
use serial_test::serial;
use tokio::sync::mpsc;

const MIB: usize = 1024 * 1024;
const TESTBED_DEADLINE_SEC: u64 = 20;

#[ctor]
fn init() {
    let _ = dotenvy::from_filename("./tests/.env");
}

fn get_tb_builder() -> TestBedBuilder {
    TestBedBuilder::new("./tests/fixture/main_with_s3fs").with_oneshot_policy(None)
}

async fn remove(path: &str, recursive: bool) {
    let tb = get_tb_builder().build().await;
    let resp = tb
        .request(|b| {
            b.uri(format!("/remove/{}?recursive={}", path, recursive))
                .method("GET")
                .body(Body::empty())
                .context("can't make request")
        })
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), StatusCode::OK);
    tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
}

async fn test_write_and_get_bytes(bytes: usize) {
    remove("", true).await;

    let mut arr = vec![0u8; bytes];

    {
        let tb = get_tb_builder()
            .with_server_flags(ServerFlags {
                request_buffer_size: Some(64 * 1024),
                ..Default::default()
            })
            .build()
            .await;

        rand::thread_rng().fill_bytes(&mut arr);

        let resp = tb
            .request(|b| {
                b.uri("/write/meow.bin")
                    .method("POST")
                    .body(arr.clone().into())
                    .context("can't make request")
            })
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), StatusCode::OK);
        tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
    }

    {
        let tb = get_tb_builder().build().await;
        let mut resp = tb
            .request(|b| {
                b.uri("/get/meow.bin")
                    .method("GET")
                    .body(Body::empty())
                    .context("can't make request")
            })
            .await
            .unwrap();

        let buf = to_bytes(resp.body_mut()).await.unwrap();
        let buf = buf.as_ref();

        assert_eq!(resp.status().as_u16(), StatusCode::OK);
        assert_eq!(arr, buf);
        tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
    }
}

#[tokio::test]
#[serial]
async fn test_write_and_get_various_bytes() {
    test_write_and_get_bytes(0).await;
    test_write_and_get_bytes(1).await;
    test_write_and_get_bytes(3 * MIB).await;
    test_write_and_get_bytes(5 * MIB).await;
    test_write_and_get_bytes(8 * MIB).await;
    test_write_and_get_bytes(50 * MIB).await;
}

/// This test is to ensure that the Upload file size limit in the storage settings section is
/// working properly.
///
/// Note that the test below assumes an upload file size limit of 50 MiB.
///
/// See: https://supabase.com/docs/guides/storage/uploads/file-limits
#[tokio::test]
#[serial]
async fn test_write_and_get_over_50_mib() {
    remove("", true).await;

    {
        let arr = vec![0u8; 51 * MIB];
        let tb = get_tb_builder()
            .with_server_flags(ServerFlags {
                request_buffer_size: Some(64 * 1024),
                ..Default::default()
            })
            .build()
            .await;

        let resp = tb
            .request(|b| {
                b.uri("/write/meow.bin")
                    .method("POST")
                    .body(arr.clone().into())
                    .context("can't make request")
            })
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), StatusCode::OK);
        tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
    }

    {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let tb = get_tb_builder()
            .with_worker_event_sender(Some(tx))
            .build()
            .await;

        let resp = tb
            .request(|b| {
                b.uri("/get/meow.bin")
                    .method("GET")
                    .body(Body::empty())
                    .context("can't make request")
            })
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), StatusCode::INTERNAL_SERVER_ERROR);

        tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;

        let mut found_not_found_error = false;

        while let Some(ev) = rx.recv().await {
            let WorkerEvents::Log(ev) = ev.event else {
                continue;
            };
            if ev.level != LogLevel::Error {
                continue;
            }

            found_not_found_error = ev.msg.contains("NotFound: entity not found: open '/s3/");
            if found_not_found_error {
                break;
            }
        }

        assert!(found_not_found_error);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DenoDirEntry {
    name: String,
    is_file: bool,
    is_directory: bool,
}

impl DenoDirEntry {
    fn from_json_unchecked(slice: &[u8]) -> HashMap<String, Self> {
        serde_json::from_slice::<Vec<DenoDirEntry>>(slice)
            .unwrap()
            .into_iter()
            .map(|it| (it.name.clone(), it))
            .collect()
    }
}

#[tokio::test]
#[serial]
async fn test_mkdir_and_read_dir() {
    remove("", true).await;

    {
        let tb = get_tb_builder().build().await;
        let resp = tb
            .request(|b| {
                b.uri("/mkdir/a")
                    .method("GET")
                    .body(Body::empty())
                    .context("can't make request")
            })
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), StatusCode::OK);
        tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
    }

    {
        let tb = get_tb_builder().build().await;
        let mut resp = tb
            .request(|b| {
                b.uri("/read-dir")
                    .method("GET")
                    .body(Body::empty())
                    .context("can't make request")
            })
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), StatusCode::OK);

        let buf = to_bytes(resp.body_mut()).await.unwrap();
        let value = DenoDirEntry::from_json_unchecked(&buf);

        assert!(value.contains_key("a"));
        assert!(value.get("a").unwrap().is_directory);

        tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
    }
}

#[tokio::test]
#[serial]
async fn test_mkdir_recursive_and_read_dir() {
    remove("", true).await;

    {
        let tb = get_tb_builder().build().await;
        let resp = tb
            .request(|b| {
                b.uri("/mkdir/a/b/c/meow?recursive=true")
                    .method("GET")
                    .body(Body::empty())
                    .context("can't make request")
            })
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), StatusCode::OK);
        tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
    }

    {
        let tb = get_tb_builder().build().await;

        for [dir, expected] in [["", "a"], ["a", "b"], ["a/b", "c"], ["a/b/c", "meow"]] {
            let mut resp = tb
                .request(|b| {
                    b.uri(format!("/read-dir/{}", dir))
                        .method("GET")
                        .body(Body::empty())
                        .context("can't make request")
                })
                .await
                .unwrap();

            assert_eq!(resp.status().as_u16(), StatusCode::OK);

            let buf = to_bytes(resp.body_mut()).await.unwrap();
            let value = DenoDirEntry::from_json_unchecked(&buf);

            assert!(value.contains_key(expected));
            assert!(value.get(expected).unwrap().is_directory);
        }

        tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
    }
}

#[tokio::test]
#[serial]
async fn test_mkdir_with_no_recursive_opt_must_check_parent_path_exists() {
    remove("", true).await;

    {
        let tb = get_tb_builder().build().await;
        let resp = tb
            .request(|b| {
                b.uri("/mkdir/a")
                    .method("GET")
                    .body(Body::empty())
                    .context("can't make request")
            })
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), StatusCode::OK);
        tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
    }

    {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let tb = get_tb_builder()
            .with_worker_event_sender(Some(tx))
            .build()
            .await;
        let resp = tb
            .request(|b| {
                b.uri("/mkdir/a/b/c")
                    .method("GET")
                    .body(Body::empty())
                    .context("can't make request")
            })
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), StatusCode::INTERNAL_SERVER_ERROR);
        tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;

        let mut found_no_such_file_or_directory_error = false;

        while let Some(ev) = rx.recv().await {
            let WorkerEvents::Log(ev) = ev.event else {
                continue;
            };
            if ev.level != LogLevel::Error {
                continue;
            }

            found_no_such_file_or_directory_error =
                ev.msg.contains("No such file or directory: a/b");

            if found_no_such_file_or_directory_error {
                break;
            }
        }

        assert!(found_no_such_file_or_directory_error);
    }
}

#[tokio::test]
#[serial]
async fn test_mkdir_recursive_and_remove_recursive() {
    remove("", true).await;

    {
        let tb = get_tb_builder().build().await;
        let resp = tb
            .request(|b| {
                b.uri("/mkdir/a/b/c/meow?recursive=true")
                    .method("GET")
                    .body(Body::empty())
                    .context("can't make request")
            })
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), StatusCode::OK);
        tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
    }

    {
        let arr = vec![0u8; 11 * MIB];
        let tb = get_tb_builder()
            .with_server_flags(ServerFlags {
                request_buffer_size: Some(64 * 1024),
                ..Default::default()
            })
            .build()
            .await;

        let resp = tb
            .request(|b| {
                b.uri("/write/a/b/c/meeeeow.bin")
                    .method("POST")
                    .body(arr.clone().into())
                    .context("can't make request")
            })
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), StatusCode::OK);
        tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
    }

    {
        let tb = get_tb_builder().build().await;
        let mut resp = tb
            .request(|b| {
                b.uri("/read-dir/a/b/c")
                    .method("GET")
                    .body(Body::empty())
                    .context("can't make request")
            })
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), StatusCode::OK);

        let buf = to_bytes(resp.body_mut()).await.unwrap();
        let value = DenoDirEntry::from_json_unchecked(&buf);

        assert_eq!(
            value.len(),
            if is_supabase_storage_being_tested() {
                // .emptyFolderPlaceholder in Supabase Storage
                3
            } else {
                2
            }
        );

        assert!(value.contains_key("meow"));
        assert!(value.get("meow").unwrap().is_directory);
        assert!(value.contains_key("meeeeow.bin"));
        assert!(value.get("meeeeow.bin").unwrap().is_file);

        tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
    }

    remove("a/b/c", true).await;
    remove("a/b", true).await;

    {
        let tb = get_tb_builder().build().await;
        let mut resp = tb
            .request(|b| {
                b.uri("/read-dir/a")
                    .method("GET")
                    .body(Body::empty())
                    .context("can't make request")
            })
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), StatusCode::OK);

        let buf = to_bytes(resp.body_mut()).await.unwrap();
        let value = DenoDirEntry::from_json_unchecked(&buf);

        assert_eq!(
            value.len(),
            if is_supabase_storage_being_tested() {
                // .emptyFolderPlaceholder in Supabase Storage
                1
            } else {
                0
            }
        );

        tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
    }

    {
        let tb = get_tb_builder().build().await;
        let mut resp = tb
            .request(|b| {
                b.uri("/read-dir")
                    .method("GET")
                    .body(Body::empty())
                    .context("can't make request")
            })
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), StatusCode::OK);

        let buf = to_bytes(resp.body_mut()).await.unwrap();
        let value = DenoDirEntry::from_json_unchecked(&buf);

        assert_eq!(value.len(), 1);
        assert!(value.contains_key("a"));
        assert!(value.get("a").unwrap().is_directory);

        tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
    }

    remove("a", true).await;

    {
        let tb = get_tb_builder().build().await;
        let mut resp = tb
            .request(|b| {
                b.uri("/read-dir")
                    .method("GET")
                    .body(Body::empty())
                    .context("can't make request")
            })
            .await
            .unwrap();

        assert_eq!(resp.status().as_u16(), StatusCode::OK);

        let buf = to_bytes(resp.body_mut()).await.unwrap();
        let value = DenoDirEntry::from_json_unchecked(&buf);

        assert_eq!(value.len(), 0);

        tb.exit(Duration::from_secs(TESTBED_DEADLINE_SEC)).await;
    }
}

fn is_supabase_storage_being_tested() -> bool {
    std::env::var("SFFS_TEST_SUPABASE_STORAGE").unwrap_or_default() == "true"
}
