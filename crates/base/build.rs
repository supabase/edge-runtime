// build script
use std::env;
use std::path::PathBuf;

mod supabase_startup_snapshot {
    use super::*;
    use deno_core::error::AnyError;
    use deno_core::snapshot::{create_snapshot, CreateSnapshotOptions};
    use deno_core::Extension;
    use deno_fs::OpenOptions;
    use deno_http::DefaultHttpPropertyExtractor;
    use deno_io::fs::FsError;
    use event_worker::js_interceptors::sb_events_js_interceptors;
    use event_worker::sb_user_event_worker;
    use sb_ai::sb_ai;
    use sb_core::http::sb_core_http;
    use sb_core::http_start::sb_core_http_start;
    use sb_core::net::sb_core_net;
    use sb_core::permissions::sb_core_permissions;
    use sb_core::runtime::sb_core_runtime;
    use sb_core::sb_core_main_js;
    use sb_core::transpiler::maybe_transpile_source;
    use sb_env::sb_env;
    use sb_node::deno_node;
    use sb_workers::sb_user_workers;
    use std::borrow::Cow;
    use std::io::Write;
    use std::path::Path;
    use std::rc::Rc;
    use std::sync::Arc;
    use url::Url;

    #[derive(Clone)]
    pub struct Permissions;

    impl deno_net::NetPermissions for Permissions {
        fn check_net<T: AsRef<str>>(
            &mut self,
            _host: &(T, Option<u16>),
            _api_name: &str,
        ) -> Result<(), deno_core::error::AnyError> {
            unreachable!("snapshotting!")
        }

        fn check_read(
            &mut self,
            _p: &Path,
            _api_name: &str,
        ) -> Result<(), deno_core::error::AnyError> {
            unreachable!("snapshotting!")
        }

        fn check_write(
            &mut self,
            _p: &Path,
            _api_name: &str,
        ) -> Result<(), deno_core::error::AnyError> {
            unreachable!("snapshotting!")
        }
    }

    impl deno_fetch::FetchPermissions for Permissions {
        fn check_net_url(
            &mut self,
            _url: &deno_core::url::Url,
            _api_name: &str,
        ) -> Result<(), deno_core::error::AnyError> {
            unreachable!("snapshotting!")
        }

        fn check_read(
            &mut self,
            _p: &Path,
            _api_name: &str,
        ) -> Result<(), deno_core::error::AnyError> {
            unreachable!("snapshotting!")
        }
    }

    impl deno_web::TimersPermission for Permissions {
        fn allow_hrtime(&mut self) -> bool {
            unreachable!("snapshotting!")
        }
    }

    impl deno_websocket::WebSocketPermissions for Permissions {
        fn check_net_url(
            &mut self,
            _url: &deno_core::url::Url,
            _api_name: &str,
        ) -> Result<(), deno_core::error::AnyError> {
            unreachable!("snapshotting!")
        }
    }

    impl deno_fs::FsPermissions for Permissions {
        fn check_open<'a>(
            &mut self,
            _resolved: bool,
            _read: bool,
            _write: bool,
            path: &'a Path,
            _api_name: &str,
        ) -> Result<Cow<'a, Path>, FsError> {
            Ok(Cow::Borrowed(path))
        }

        fn check_read(&mut self, _path: &Path, _api_name: &str) -> Result<(), AnyError> {
            unreachable!("snapshotting!")
        }

        fn check_read_all(&mut self, _api_name: &str) -> Result<(), AnyError> {
            unreachable!("snapshotting!")
        }

        fn check_read_blind(
            &mut self,
            _path: &Path,
            _display: &str,
            _api_name: &str,
        ) -> Result<(), AnyError> {
            unreachable!("snapshotting!")
        }

        fn check_write(&mut self, _path: &Path, _api_name: &str) -> Result<(), AnyError> {
            unreachable!("snapshotting!")
        }

        fn check_write_partial(&mut self, _path: &Path, _api_name: &str) -> Result<(), AnyError> {
            unreachable!("snapshotting!")
        }

        fn check_write_all(&mut self, _api_name: &str) -> Result<(), AnyError> {
            unreachable!("snapshotting!")
        }

        fn check_write_blind(
            &mut self,
            _p: &Path,
            _display: &str,
            _api_name: &str,
        ) -> Result<(), AnyError> {
            unreachable!("snapshotting!")
        }

        fn check<'a>(
            &mut self,
            _resolved: bool,
            _open_options: &OpenOptions,
            _path: &'a Path,
            _api_name: &str,
        ) -> Result<std::borrow::Cow<'a, Path>, FsError> {
            unreachable!("snapshotting!")
        }
    }

    impl sb_node::NodePermissions for Permissions {
        fn check_net_url(&mut self, _url: &Url, _api_name: &str) -> Result<(), AnyError> {
            unreachable!("snapshotting!")
        }

        fn check_read(&self, _path: &Path) -> Result<(), AnyError> {
            unreachable!("snapshotting!")
        }

        fn check_read_with_api_name(
            &self,
            _path: &Path,
            _api_name: Option<&str>,
        ) -> Result<(), AnyError> {
            unreachable!("snapshotting!")
        }

        fn check_sys(&self, _kind: &str, _api_name: &str) -> Result<(), AnyError> {
            unreachable!("snapshotting!")
        }

        fn check_write_with_api_name(
            &self,
            _path: &Path,
            _api_name: Option<&str>,
        ) -> Result<(), AnyError> {
            unreachable!("snapshotting!")
        }
    }

    pub fn create_runtime_snapshot(snapshot_path: PathBuf) {
        let user_agent = String::from("supabase");
        let fs = Arc::new(deno_fs::RealFs);
        let extensions: Vec<Extension> = vec![
            sb_core_permissions::init_ops_and_esm(false),
            deno_webidl::deno_webidl::init_ops_and_esm(),
            deno_console::deno_console::init_ops_and_esm(),
            deno_url::deno_url::init_ops_and_esm(),
            deno_web::deno_web::init_ops_and_esm::<Permissions>(
                Arc::new(deno_web::BlobStore::default()),
                None,
            ),
            deno_webgpu::deno_webgpu::init_ops_and_esm(),
            deno_canvas::deno_canvas::init_ops_and_esm(),
            deno_fetch::deno_fetch::init_ops_and_esm::<Permissions>(deno_fetch::Options {
                user_agent: user_agent.clone(),
                root_cert_store_provider: None,
                ..Default::default()
            }),
            deno_websocket::deno_websocket::init_ops_and_esm::<Permissions>(user_agent, None, None),
            // TODO: support providing a custom seed for crypto
            deno_crypto::deno_crypto::init_ops_and_esm(None),
            deno_broadcast_channel::deno_broadcast_channel::init_ops_and_esm(
                deno_broadcast_channel::InMemoryBroadcastChannel::default(),
            ),
            deno_net::deno_net::init_ops_and_esm::<Permissions>(None, None),
            deno_tls::deno_tls::init_ops_and_esm(),
            deno_http::deno_http::init_ops_and_esm::<DefaultHttpPropertyExtractor>(),
            deno_io::deno_io::init_ops_and_esm(Some(Default::default())),
            deno_fs::deno_fs::init_ops_and_esm::<Permissions>(fs.clone()),
            sb_ai::init_ops_and_esm(),
            sb_env::init_ops_and_esm(),
            sb_os::sb_os::init_ops_and_esm(),
            sb_user_workers::init_ops_and_esm(),
            sb_user_event_worker::init_ops_and_esm(),
            sb_events_js_interceptors::init_ops_and_esm(),
            sb_core_main_js::init_ops_and_esm(),
            sb_core_net::init_ops_and_esm(),
            sb_core_http::init_ops_and_esm(),
            sb_core_http_start::init_ops_and_esm(),
            deno_node::init_ops_and_esm::<Permissions>(None, fs),
            sb_core_runtime::init_ops_and_esm(None),
        ];

        let snapshot = create_snapshot(
            CreateSnapshotOptions {
                cargo_manifest_dir: env!("CARGO_MANIFEST_DIR"),
                startup_snapshot: None,
                extensions,
                extension_transpiler: Some(Rc::new(|specifier, source| {
                    maybe_transpile_source(specifier, source)
                })),
                skip_op_registration: false,
                with_runtime_cb: None,
            },
            None,
        );

        let output = snapshot.unwrap();

        let mut snapshot = std::fs::File::create(snapshot_path).unwrap();
        snapshot.write_all(&output.output).unwrap();

        for path in output.files_loaded_during_snapshot {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

fn main() {
    println!("cargo:rustc-env=TARGET={}", env::var("TARGET").unwrap());
    println!("cargo:rustc-env=PROFILE={}", env::var("PROFILE").unwrap());

    let o = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    // Main snapshot
    let runtime_snapshot_path = o.join("RUNTIME_SNAPSHOT.bin");

    supabase_startup_snapshot::create_runtime_snapshot(runtime_snapshot_path.clone());
}
