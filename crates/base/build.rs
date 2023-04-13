// build script
use std::env;
use std::fmt::format;
use std::path::PathBuf;

mod supabase_startup_snapshot {
    use super::*;
    use deno_ast::MediaType;
    use deno_ast::ParseParams;
    use deno_ast::SourceTextInfo;
    use deno_core::error::AnyError;
    use deno_core::snapshot_util::*;
    use deno_core::Extension;
    use deno_core::ExtensionFileSource;
    use deno_core::ModuleCode;
    use sb_core::http_start::sb_core_http;
    use sb_core::net::sb_core_net;
    use sb_core::permissions::sb_core_permissions;
    use sb_core::runtime::sb_core_runtime;
    use sb_core::sb_core_main_js;
    use sb_env::sb_env;
    use sb_workers::sb_user_workers;
    use std::path::Path;

    fn transpile_ts_for_snapshotting(
        file_source: &ExtensionFileSource,
    ) -> Result<ModuleCode, AnyError> {
        let media_type = MediaType::from_path(Path::new(&file_source.specifier));

        let should_transpile = match media_type {
            MediaType::JavaScript => false,
            MediaType::Mjs => false,
            MediaType::TypeScript => true,
            _ => panic!(
                "Unsupported media type for snapshotting {media_type:?} for file {}",
                file_source.specifier
            ),
        };
        let code = file_source.load()?;

        if !should_transpile {
            return Ok(code);
        }

        let parsed = deno_ast::parse_module(ParseParams {
            specifier: file_source.specifier.to_string(),
            text_info: SourceTextInfo::from_string(code.take_as_string()),
            media_type,
            capture_tokens: false,
            scope_analysis: false,
            maybe_syntax: None,
        })?;
        let transpiled_source = parsed.transpile(&deno_ast::EmitOptions {
            imports_not_used_as_values: deno_ast::ImportsNotUsedAsValues::Remove,
            inline_source_map: false,
            ..Default::default()
        })?;

        Ok(transpiled_source.text.into())
    }

    #[derive(Clone)]
    struct Permissions;

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

        fn check_unstable(&self, _state: &deno_core::OpState, _api_name: &'static str) {
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

    pub fn create_runtime_snapshot(snapshot_path: PathBuf) {
        let user_agent = String::from("supabase");
        let extensions: Vec<Extension> = vec![
            sb_core_permissions::init_ops_and_esm(),
            deno_webidl::deno_webidl::init_ops_and_esm(),
            deno_console::deno_console::init_ops_and_esm(),
            deno_url::deno_url::init_ops_and_esm(),
            deno_web::deno_web::init_ops_and_esm::<Permissions>(
                deno_web::BlobStore::default(),
                None,
            ),
            deno_fetch::deno_fetch::init_ops_and_esm::<Permissions>(deno_fetch::Options {
                user_agent: user_agent.clone(),
                root_cert_store: None,
                ..Default::default()
            }),
            deno_websocket::deno_websocket::init_ops_and_esm::<Permissions>(
                user_agent.clone(),
                None,
                None,
            ),
            // TODO: support providing a custom seed for crypto
            deno_crypto::deno_crypto::init_ops_and_esm(None),
            deno_net::deno_net::init_ops_and_esm::<Permissions>(None, false, None),
            deno_tls::deno_tls::init_ops_and_esm(),
            deno_http::deno_http::init_ops_and_esm(),
            sb_env::init_ops_and_esm(),
            sb_user_workers::init_ops_and_esm(),
            sb_core_main_js::init_ops_and_esm(),
            sb_core_net::init_ops_and_esm(),
            sb_core_http::init_ops_and_esm(),
            sb_core_runtime::init_ops_and_esm(None),
        ];

        create_snapshot(CreateSnapshotOptions {
            cargo_manifest_dir: env!("CARGO_MANIFEST_DIR"),
            snapshot_path,
            startup_snapshot: None,
            extensions,
            compression_cb: None,
            snapshot_module_load_cb: Some(Box::new(transpile_ts_for_snapshotting)),
        });
    }
}

fn main() {
    println!("cargo:rustc-env=TARGET={}", env::var("TARGET").unwrap());
    println!("cargo:rustc-env=PROFILE={}", env::var("PROFILE").unwrap());

    let o = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    // Main snapshot
    let runtime_snapshot_path = o.join("RUNTIME_SNAPSHOT.bin");

    supabase_startup_snapshot::create_runtime_snapshot(runtime_snapshot_path)
}
