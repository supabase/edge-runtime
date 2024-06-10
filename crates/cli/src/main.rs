mod env;
mod flags;

#[cfg(not(feature = "tracing"))]
mod logger;

use anyhow::{anyhow, bail, Error};
use base::commands::start_server;

use base::rt_worker::worker_pool::{SupervisorPolicy, WorkerPoolPolicy};
use base::server::{ServerFlags, Tls, WorkerEntrypoints};
use base::{DecoratorType, InspectorOption};
use clap::ArgMatches;
use deno_core::url::Url;
use env::resolve_deno_runtime_env;
use flags::get_cli;
use log::warn;
use sb_graph::emitter::EmitterFactory;
use sb_graph::import_map::load_import_map;
use sb_graph::{
    extract_from_file, generate_binary_eszip, include_glob_patterns_in_eszip, STATIC_FS_PREFIX,
};
use std::fs::File;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

fn main() -> Result<(), anyhow::Error> {
    resolve_deno_runtime_env();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .thread_name("sb-main")
        .build()
        .unwrap();

    // TODO: Tokio runtime shouldn't be needed here (Address later)
    let local = tokio::task::LocalSet::new();
    let res: Result<(), Error> = local.block_on(&runtime, async {
        let matches = get_cli().get_matches();

        if !matches.get_flag("quiet") {
            #[cfg(feature = "tracing")]
            {
                use tracing_subscriber::EnvFilter;

                tracing_subscriber::fmt()
                    .with_env_filter(EnvFilter::from_default_env())
                    .with_thread_names(true)
                    .init()
            }

            #[cfg(not(feature = "tracing"))]
            {
                let verbose = matches.get_flag("verbose");
                let include_source = matches.get_flag("log-source");
                logger::init(verbose, include_source);
            }
        }

        #[allow(clippy::single_match)]
        #[allow(clippy::arc_with_non_send_sync)]
        match matches.subcommand() {
            Some(("start", sub_matches)) => {
                let ip = sub_matches.get_one::<String>("ip").cloned().unwrap();
                let port = sub_matches.get_one::<u16>("port").copied().unwrap();

                let maybe_tls = if let Some(port) = sub_matches.get_one::<u16>("tls").copied() {
                    let Some((key_slice, cert_slice)) = sub_matches
                        .get_one::<PathBuf>("key")
                        .and_then(|it| std::fs::read(it).ok())
                        .zip(
                            sub_matches
                                .get_one::<PathBuf>("cert")
                                .and_then(|it| std::fs::read(it).ok()),
                        )
                    else {
                        bail!("unable to load the key file or cert file");
                    };

                    Some(Tls::new(port, &key_slice, &cert_slice)?)
                } else {
                    None
                };

                let main_service_path = sub_matches
                    .get_one::<String>("main-service")
                    .cloned()
                    .unwrap();
                let import_map_path = sub_matches.get_one::<String>("import-map").cloned();

                let no_module_cache = sub_matches
                    .get_one::<bool>("disable-module-cache")
                    .cloned()
                    .unwrap();

                let allow_main_inspector = sub_matches
                    .get_one::<bool>("inspect-main")
                    .cloned()
                    .unwrap();

                let event_service_manager_path =
                    sub_matches.get_one::<String>("event-worker").cloned();
                let maybe_main_entrypoint =
                    sub_matches.get_one::<String>("main-entrypoint").cloned();
                let maybe_events_entrypoint =
                    sub_matches.get_one::<String>("events-entrypoint").cloned();

                let maybe_supervisor_policy = sub_matches
                    .get_one::<String>("policy")
                    .map(|it| it.parse::<SupervisorPolicy>().unwrap());

                let graceful_exit_deadline_sec = sub_matches
                    .get_one::<u64>("graceful-exit-timeout")
                    .cloned()
                    .unwrap_or(0);

                let graceful_exit_keepalive_deadline_ms = sub_matches
                    .get_one::<u64>("experimental-graceful-exit-keepalive-deadline-ratio")
                    .cloned()
                    .and_then(|it| {
                        if it == 0 {
                            return None;
                        }

                        let deadline_ms = graceful_exit_deadline_sec * 1000;
                        let percent = std::cmp::min(it, 100) as f64;
                        let point = percent / 100.0f64;

                        if point.is_normal() {
                            Some(((deadline_ms as f64) * point) as u64)
                        } else {
                            None
                        }
                    });

                let maybe_max_parallelism =
                    sub_matches.get_one::<usize>("max-parallelism").cloned();
                let maybe_request_wait_timeout =
                    sub_matches.get_one::<u64>("request-wait-timeout").cloned();
                let maybe_request_idle_timeout =
                    sub_matches.get_one::<u64>("request-idle-timeout").cloned();
                let maybe_request_read_timeout =
                    sub_matches.get_one::<u64>("request-read-timeout").cloned();
                let static_patterns =
                    if let Some(val_ref) = sub_matches.get_many::<String>("static") {
                        val_ref.map(|s| s.as_str()).collect::<Vec<&str>>()
                    } else {
                        vec![]
                    };

                let jsx_specifier = sub_matches.get_one::<String>("jsx-specifier").cloned();
                let jsx_module = sub_matches.get_one::<String>("jsx-module").cloned();

                let static_patterns: Vec<String> =
                    static_patterns.into_iter().map(|s| s.to_string()).collect();

                let inspector = sub_matches.get_one::<clap::Id>("inspector").zip(
                    sub_matches
                        .get_one("inspect")
                        .or(sub_matches.get_one("inspect-brk"))
                        .or(sub_matches.get_one::<SocketAddr>("inspect-wait")),
                );

                let maybe_inspector_option = if let Some((key, addr)) = inspector {
                    Some(get_inspector_option(key.as_str(), addr).unwrap())
                } else {
                    None
                };

                let tcp_nodelay = sub_matches.get_one::<bool>("tcp-nodelay").copied().unwrap();
                let flags = ServerFlags {
                    no_module_cache,
                    allow_main_inspector,
                    tcp_nodelay,
                    graceful_exit_deadline_sec,
                    graceful_exit_keepalive_deadline_ms,
                    request_wait_timeout_ms: maybe_request_wait_timeout,
                    request_idle_timeout_ms: maybe_request_idle_timeout,
                    request_read_timeout_ms: maybe_request_read_timeout,
                };

                start_server(
                    ip.as_str(),
                    port,
                    maybe_tls,
                    main_service_path,
                    event_service_manager_path,
                    get_decorator_option(sub_matches),
                    Some(WorkerPoolPolicy::new(
                        maybe_supervisor_policy,
                        if let Some(true) = maybe_supervisor_policy
                            .as_ref()
                            .map(SupervisorPolicy::is_oneshot)
                        {
                            if let Some(parallelism) = maybe_max_parallelism {
                                if parallelism == 0 || parallelism > 1 {
                                    warn!(
                                        "{}",
                                        concat!(
                                            "if `oneshot` policy is enabled, the maximum ",
                                            "parallelism is fixed to `1` as forcibly"
                                        )
                                    );
                                }
                            }

                            Some(1)
                        } else {
                            maybe_max_parallelism
                        },
                        flags,
                    )),
                    import_map_path,
                    flags,
                    None,
                    WorkerEntrypoints {
                        main: maybe_main_entrypoint,
                        events: maybe_events_entrypoint,
                    },
                    None,
                    static_patterns,
                    maybe_inspector_option,
                    jsx_specifier,
                    jsx_module,
                )
                .await?;
            }
            Some(("bundle", sub_matches)) => {
                let output_path = sub_matches.get_one::<String>("output").cloned().unwrap();
                let import_map_path = sub_matches.get_one::<String>("import-map").cloned();
                let maybe_decorator = get_decorator_option(sub_matches);
                let static_patterns =
                    if let Some(val_ref) = sub_matches.get_many::<String>("static") {
                        val_ref.map(|s| s.as_str()).collect::<Vec<&str>>()
                    } else {
                        vec![]
                    };

                let entry_point_path = sub_matches
                    .get_one::<String>("entrypoint")
                    .cloned()
                    .unwrap();

                let path = PathBuf::from(entry_point_path.as_str());
                if !path.exists() {
                    bail!("entrypoint path does not exist ({})", path.display());
                }

                let mut emitter_factory = EmitterFactory::new();
                let maybe_import_map = load_import_map(import_map_path.clone())
                    .map_err(|e| anyhow!("import map path is invalid ({})", e))?;
                let mut maybe_import_map_url = None;
                if maybe_import_map.is_some() {
                    let abs_import_map_path =
                        std::env::current_dir().map(|p| p.join(import_map_path.unwrap()))?;
                    maybe_import_map_url = Some(
                        Url::from_file_path(abs_import_map_path)
                            .map_err(|_| anyhow!("failed get import map url"))?
                            .to_string(),
                    );
                }

                emitter_factory.set_decorator_type(maybe_decorator);
                emitter_factory.set_import_map(maybe_import_map.clone());

                let mut eszip = generate_binary_eszip(
                    path.canonicalize().unwrap(),
                    Arc::new(emitter_factory),
                    None,
                    maybe_import_map_url,
                )
                .await?;

                include_glob_patterns_in_eszip(
                    static_patterns,
                    &mut eszip,
                    Some(STATIC_FS_PREFIX.to_string()),
                )
                .await;

                let bin = eszip.into_bytes();

                if output_path == "-" {
                    let stdout = std::io::stdout();
                    let mut handle = stdout.lock();

                    handle.write_all(&bin)?
                } else {
                    let mut file = File::create(output_path.as_str())?;
                    file.write_all(&bin)?
                }
            }
            Some(("unbundle", sub_matches)) => {
                let output_path = sub_matches.get_one::<String>("output").cloned().unwrap();
                let eszip_path = sub_matches.get_one::<String>("eszip").cloned().unwrap();

                let output_path = PathBuf::from(output_path.as_str());
                let eszip_path = PathBuf::from(eszip_path.as_str());

                extract_from_file(eszip_path, output_path.clone()).await;

                println!(
                    "Eszip extracted successfully inside path {}",
                    output_path.to_str().unwrap()
                );
            }
            _ => {
                // unrecognized command
            }
        }
        Ok(())
    });

    res
}

fn get_decorator_option(sub_matches: &ArgMatches) -> Option<DecoratorType> {
    sub_matches
        .get_one::<String>("decorator")
        .cloned()
        .and_then(|it| match it.to_lowercase().as_str() {
            "tc39" => Some(DecoratorType::Tc39),
            "typescript" => Some(DecoratorType::Typescript),
            "typescript_with_metadata" => Some(DecoratorType::TypescriptWithMetadata),
            _ => None,
        })
}

fn get_inspector_option(key: &str, addr: &SocketAddr) -> Result<InspectorOption, anyhow::Error> {
    match key {
        "inspect" => Ok(InspectorOption::Inspect(*addr)),
        "inspect-brk" => Ok(InspectorOption::WithBreak(*addr)),
        "inspect-wait" => Ok(InspectorOption::WithWait(*addr)),
        key => bail!("invalid inspector key: {}", key),
    }
}
