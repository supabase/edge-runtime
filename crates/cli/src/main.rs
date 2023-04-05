mod logger;

use anyhow::Error;
use base::commands::start_server;
use clap::builder::FalseyValueParser;
use clap::{arg, value_parser, ArgAction, ArgMatches, Command};
fn cli() -> Command {
    Command::new("edge-runtime")
        .about("A server based on Deno runtime, capable of running JavaScript, TypeScript, and WASM services")
        .version("0.0.1") // TODO: set version on compile time
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
        .subcommand(
            Command::new("start")
                .about("Start the server")
                .arg(arg!(-i --ip <HOST> "Host IP address to listen on").default_value("0.0.0.0"))
                .arg(
                    arg!(-p --port <PORT> "Port to listen on")
                        .default_value("9000")
                        .value_parser(value_parser!(u16)),
                )
                .arg(arg!(--"main-service" <DIR> "Path to main service directory").default_value("examples/main"))
                .arg(arg!(--"disable-module-cache" "Disable using module cache").default_value("false").value_parser(FalseyValueParser::new()))
                .arg(arg!(--"import-map" <Path> "Path to import map file"))
        )
}

async fn exit_with_code(result: Result<(), Error>) {
    match result {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            eprintln!("{:?}", error);
            std::process::exit(1)
        }
    }
}

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
            logger::init(verbose);
        }

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

                exit_with_code(start_server(
                    &ip.as_str(),
                    port,
                    main_service_path,
                    import_map_path,
                    no_module_cache,
                ))
            }
            _ => {
                // unrecognized command
            }
        }
        Ok(())
    });

    res
}
