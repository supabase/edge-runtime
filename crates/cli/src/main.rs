mod logger;

use anyhow::Error;
use base::commands::start_server;
use base::server::WorkerEntrypoints;
use base::utils::graph_util::{create_eszip_from_graph, create_module_graph_from_path};
use clap::builder::{BoolValueParser, FalseyValueParser};
use clap::{arg, crate_version, value_parser, ArgAction, Command};
use std::fs::File;
use std::io::Write;

fn cli() -> Command {
    Command::new("edge-runtime")
        .about("A server based on Deno runtime, capable of running JavaScript, TypeScript, and WASM services")
        .version(crate_version!())
        .arg_required_else_help(true)
        .arg(
            arg!(-v --verbose "Use verbose output")
                .conflicts_with("quiet")
                .global(true)
                .action(ArgAction::SetTrue),
        )
        .arg(
            arg!(-q --quiet "Do not print any log messages")
                .conflicts_with("verbose")
                .global(true)
                .action(ArgAction::SetTrue),
        )
        .arg(
            arg!(--"log-source" "Include source file and line in log messages")
                .global(true)
                .action(ArgAction::SetTrue),
        )
        .subcommand(
            Command::new("start")
                .about("Start the server")
                .arg(arg!(-i --ip <HOST> "Host IP address to listen on").default_value("0.0.0.0"))
                .arg(
                    arg!(-p --port <PORT> "Port to listen on")
                        .default_value("9000")
                        .value_parser(value_parser!(u16)),
                )
                .arg(arg!(--"main-service" <DIR> "Path to main service directory or eszip").default_value("examples/main"))
                .arg(arg!(--"disable-module-cache" "Disable using module cache").default_value("false").value_parser(FalseyValueParser::new()))
                .arg(arg!(--"watch" "Watch files for hot reloading").default_value("true").value_parser(BoolValueParser::new()))
                .arg(arg!(--"import-map" <Path> "Path to import map file"))
                .arg(arg!(--"event-worker" <Path> "Path to event worker directory"))
                .arg(arg!(--"main-entrypoint" <Path> "Path to entrypoint in main service (only for eszips)"))
                .arg(arg!(--"events-entrypoint" <Path> "Path to entrypoint in events worker (only for eszips)"))
        )
        .subcommand(
            Command::new("bundle")
                .about("Creates an 'eszip' file that can be executed by the EdgeRuntime. Such file contains all the modules in contained in a single binary.")
                .arg(arg!(--"output" <DIR> "Path to output eszip file").default_value("bin.eszip"))
                .arg(arg!(--"entrypoint" <Path> "Path to entrypoint to bundle as an eszip").required(true))
        )
}

//async fn exit_with_code(result: Result<(), Error>) {
//    match result {
//        Ok(()) => std::process::exit(0),
//        Err(error) => {
//            eprintln!("{:?}", error);
//            std::process::exit(1)
//        }
//    }
//}

fn main() -> Result<(), anyhow::Error> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // TODO: Tokio runtime shouldn't be needed here (Address later)
    let local = tokio::task::LocalSet::new();
    let res: Result<(), Error> = local.block_on(&runtime, async {
        let matches = cli().get_matches();

        if !matches.get_flag("quiet") {
            let verbose = matches.get_flag("verbose");
            let include_source = matches.get_flag("log-source");
            logger::init(verbose, include_source);
        }

        #[allow(clippy::single_match)]
        match matches.subcommand() {
            Some(("start", sub_matches)) => {
                let ip = sub_matches.get_one::<String>("ip").cloned().unwrap();
                let port = sub_matches.get_one::<u16>("port").copied().unwrap();

                let main_service_path = sub_matches
                    .get_one::<String>("main-service")
                    .cloned()
                    .unwrap();
                let import_map_path = sub_matches.get_one::<String>("import-map").cloned();
                let no_module_cache = sub_matches
                    .get_one::<bool>("disable-module-cache")
                    .cloned()
                    .unwrap();
                let event_service_manager_path =
                    sub_matches.get_one::<String>("event-worker").cloned();
                let maybe_main_entrypoint =
                    sub_matches.get_one::<String>("main-entrypoint").cloned();
                let maybe_events_entrypoint =
                    sub_matches.get_one::<String>("events-entrypoint").cloned();
                let watch = sub_matches.get_one::<bool>("watch").cloned().unwrap();

                start_server(
                    ip.as_str(),
                    port,
                    main_service_path,
                    event_service_manager_path,
                    import_map_path,
                    no_module_cache,
                    None,
                    WorkerEntrypoints {
                        main: maybe_main_entrypoint,
                        events: maybe_events_entrypoint,
                    },
                )
                .await?;
            }
            Some(("bundle", sub_matches)) => {
                let output_path = sub_matches.get_one::<String>("output").cloned().unwrap();

                let entry_point_path = sub_matches
                    .get_one::<String>("entrypoint")
                    .cloned()
                    .unwrap();

                let create_graph_from_path =
                    create_module_graph_from_path(entry_point_path.as_str())
                        .await
                        .unwrap();
                let create_eszip = create_eszip_from_graph(create_graph_from_path).await;
                let mut file = File::create(output_path.as_str()).unwrap();
                file.write_all(&create_eszip).unwrap();
            }
            _ => {
                // unrecognized command
            }
        }
        Ok(())
    });

    res
}
