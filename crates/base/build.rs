use std::env;
use std::path::PathBuf;

mod supabase_startup_snapshot {
  use std::borrow::Cow;
  use std::io::Write;
  use std::path::Path;
  use std::rc::Rc;
  use std::sync::Arc;

  use deno::deno_fs::OpenOptions;
  use deno::deno_http::DefaultHttpPropertyExtractor;
  use deno::deno_io::fs::FsError;
  use deno::deno_permissions::PermissionCheckError;
  use deno::runtime::shared::maybe_transpile_source;
  use deno::PermissionsContainer;
  use deno_cache::SqliteBackedCache;
  use deno_core::snapshot::create_snapshot;
  use deno_core::snapshot::CreateSnapshotOptions;
  use deno_core::url::Url;
  use deno_core::Extension;

  use super::*;

  #[derive(Clone)]
  pub struct Permissions;

  impl deno::deno_fetch::FetchPermissions for Permissions {
    fn check_net_url(
      &mut self,
      _url: &Url,
      _api_name: &str,
    ) -> Result<(), PermissionCheckError> {
      unreachable!("snapshotting!")
    }

    fn check_read<'a>(
      &mut self,
      _p: &'a Path,
      _api_name: &str,
    ) -> Result<Cow<'a, Path>, PermissionCheckError> {
      unreachable!("snapshotting!")
    }
  }

  impl deno::deno_web::TimersPermission for Permissions {
    fn allow_hrtime(&mut self) -> bool {
      unreachable!("snapshotting!")
    }
  }

  impl deno::deno_websocket::WebSocketPermissions for Permissions {
    fn check_net_url(
      &mut self,
      _url: &Url,
      _api_name: &str,
    ) -> Result<(), PermissionCheckError> {
      unreachable!("snapshotting!")
    }
  }

  impl ext_node::NodePermissions for Permissions {
    fn check_net_url(
      &mut self,
      _url: &Url,
      _api_name: &str,
    ) -> Result<(), PermissionCheckError> {
      unreachable!("snapshotting!")
    }

    fn check_net(
      &mut self,
      _host: (&str, Option<u16>),
      _api_name: &str,
    ) -> Result<(), PermissionCheckError> {
      unreachable!("snapshotting!")
    }

    fn check_read_path<'a>(
      &mut self,
      _path: &'a Path,
    ) -> Result<Cow<'a, Path>, PermissionCheckError> {
      unreachable!("snapshotting!")
    }

    fn check_read_with_api_name(
      &mut self,
      _path: &str,
      _api_name: Option<&str>,
    ) -> Result<PathBuf, PermissionCheckError> {
      unreachable!("snapshotting!")
    }

    fn query_read_all(&mut self) -> bool {
      unreachable!("snapshotting!")
    }

    fn check_write_with_api_name(
      &mut self,
      _path: &str,
      _api_name: Option<&str>,
    ) -> Result<PathBuf, PermissionCheckError> {
      unreachable!("snapshotting!")
    }

    fn check_sys(
      &mut self,
      _kind: &str,
      _api_name: &str,
    ) -> Result<(), PermissionCheckError> {
      unreachable!("snapshotting!")
    }
  }

  impl deno::deno_net::NetPermissions for Permissions {
    fn check_net<T: AsRef<str>>(
      &mut self,
      _host: &(T, Option<u16>),
      _api_name: &str,
    ) -> Result<(), PermissionCheckError> {
      unreachable!("snapshotting!")
    }

    fn check_read(
      &mut self,
      _p: &str,
      _api_name: &str,
    ) -> Result<PathBuf, PermissionCheckError> {
      unreachable!("snapshotting!")
    }

    fn check_write(
      &mut self,
      _p: &str,
      _api_name: &str,
    ) -> Result<PathBuf, PermissionCheckError> {
      unreachable!("snapshotting!")
    }

    fn check_write_path<'a>(
      &mut self,
      _p: &'a Path,
      _api_name: &str,
    ) -> Result<Cow<'a, Path>, PermissionCheckError> {
      unreachable!("snapshotting!")
    }
  }

  impl deno::deno_fs::FsPermissions for Permissions {
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

    fn check_read(
      &mut self,
      _path: &str,
      _api_name: &str,
    ) -> Result<PathBuf, PermissionCheckError> {
      unreachable!("snapshotting!")
    }

    fn check_read_all(
      &mut self,
      _api_name: &str,
    ) -> Result<(), PermissionCheckError> {
      unreachable!("snapshotting!")
    }

    fn check_read_blind(
      &mut self,
      _path: &Path,
      _display: &str,
      _api_name: &str,
    ) -> Result<(), PermissionCheckError> {
      unreachable!("snapshotting!")
    }

    fn check_write(
      &mut self,
      _path: &str,
      _api_name: &str,
    ) -> Result<PathBuf, PermissionCheckError> {
      unreachable!("snapshotting!")
    }

    fn check_write_partial(
      &mut self,
      _path: &str,
      _api_name: &str,
    ) -> Result<PathBuf, PermissionCheckError> {
      unreachable!("snapshotting!")
    }

    fn check_write_all(
      &mut self,
      _api_name: &str,
    ) -> Result<(), PermissionCheckError> {
      unreachable!("snapshotting!")
    }

    fn check_write_blind(
      &mut self,
      _p: &Path,
      _display: &str,
      _api_name: &str,
    ) -> Result<(), PermissionCheckError> {
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

    fn check_read_path<'a>(
      &mut self,
      _path: &'a Path,
      _api_name: &str,
    ) -> Result<Cow<'a, Path>, PermissionCheckError> {
      unreachable!("snapshotting!")
    }

    fn check_write_path<'a>(
      &mut self,
      _path: &'a Path,
      _api_name: &str,
    ) -> Result<Cow<'a, Path>, PermissionCheckError> {
      unreachable!("snapshotting!")
    }
  }

  pub fn create_runtime_snapshot(snapshot_path: PathBuf) {
    let user_agent = String::from("supabase");
    let fs = Arc::new(deno::deno_fs::RealFs);
    let extensions: Vec<Extension> = vec![
      deno_webidl::deno_webidl::init_ops_and_esm(),
      deno_console::deno_console::init_ops_and_esm(),
      deno::deno_url::deno_url::init_ops_and_esm(),
      deno::deno_web::deno_web::init_ops_and_esm::<Permissions>(
        Arc::new(deno::deno_web::BlobStore::default()),
        None,
      ),
      deno_webgpu::deno_webgpu::init_ops_and_esm(),
      deno_canvas::deno_canvas::init_ops_and_esm(),
      deno::deno_fetch::deno_fetch::init_ops_and_esm::<Permissions>(
        deno::deno_fetch::Options {
          user_agent: user_agent.clone(),
          root_cert_store_provider: None,
          ..Default::default()
        },
      ),
      deno::deno_websocket::deno_websocket::init_ops_and_esm::<Permissions>(
        user_agent, None, None,
      ),
      // TODO: support providing a custom seed for crypto
      deno::deno_crypto::deno_crypto::init_ops_and_esm(None),
      deno_broadcast_channel::deno_broadcast_channel::init_ops_and_esm(
        deno_broadcast_channel::InMemoryBroadcastChannel::default(),
      ),
      deno::deno_net::deno_net::init_ops_and_esm::<Permissions>(None, None),
      deno::deno_tls::deno_tls::init_ops_and_esm(),
      deno::deno_http::deno_http::init_ops_and_esm::<
        DefaultHttpPropertyExtractor,
      >(deno::deno_http::Options::default()),
      deno::deno_io::deno_io::init_ops_and_esm(Some(Default::default())),
      deno::deno_fs::deno_fs::init_ops_and_esm::<Permissions>(fs.clone()),
      ext_ai::ai::init_ops_and_esm(),
      ext_env::env::init_ops_and_esm(),
      ext_os::os::init_ops_and_esm(),
      ext_workers::user_workers::init_ops_and_esm(),
      ext_event_worker::user_event_worker::init_ops_and_esm(),
      ext_event_worker::js_interceptors::js_interceptors::init_ops_and_esm(),
      ext_core::core_main_js::init_ops_and_esm(),
      ext_core::net::core_net::init_ops_and_esm(),
      ext_core::http::core_http::init_ops_and_esm(),
      ext_core::http_start::core_http_start::init_ops_and_esm(),
      ext_node::deno_node::init_ops_and_esm::<Permissions>(None, fs),
      // NOTE(kallebysantos):
      // Full `Web Cache API` via `SqliteBackedCache` is disabled. Cache flow is
      // handled by `ext_ai: Cache Adapter`
      deno_cache::deno_cache::init_ops_and_esm::<SqliteBackedCache>(None),
      ext_core::runtime::core_runtime::init_ops_and_esm::<PermissionsContainer>(
        None,
      ),
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

  supabase_startup_snapshot::create_runtime_snapshot(
    runtime_snapshot_path.clone(),
  );
}
