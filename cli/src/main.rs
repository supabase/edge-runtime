mod logger;

use anyhow::Error;
use base::commands::start_server;
use clap::builder::FalseyValueParser;
use clap::{arg, value_parser, ArgAction, Command};

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
                .arg(arg!(--dir <DIR> "Path to services directory").default_value("services"))
                .arg(arg!(--"memory-limit" <MiB> "Memory limit for a service (in MiB)").default_value("150").value_parser(value_parser!(u16)))
                .arg(arg!(--"service-timeout" <secs> "Wall clock duration a service can run in seconds").default_value("60").value_parser(value_parser!(u16)))
                .arg(arg!(--"disable-module-cache" "Disable using module cache").default_value("false").value_parser(FalseyValueParser::new()))
                .arg(arg!(--"import-map" <Path> "Path to import map file"))
        )
}

fn exit_with_code(result: Result<(), Error>) {
    match result {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            eprintln!("{:?}", error);
            std::process::exit(1)
        }
    }
}

fn main() {
    // TODO: set panic hook

    let matches = cli().get_matches();

    if !matches.get_flag("quiet") {
        let verbose = matches.get_flag("verbose");
        logger::init(verbose);
    }

    match matches.subcommand() {
        Some(("start", sub_matches)) => {
            let ip = sub_matches.get_one::<String>("ip").cloned().unwrap();
            let port = sub_matches.get_one::<u16>("port").copied().unwrap();
            let services_dir = sub_matches.get_one::<String>("dir").cloned().unwrap();
            let service_timeout = sub_matches
                .get_one::<u16>("service-timeout")
                .cloned()
                .unwrap();
            let mem_limit = sub_matches.get_one::<u16>("memory-limit").cloned().unwrap();
            let no_module_cache = sub_matches
                .get_one::<bool>("disable-module-cache")
                .cloned()
                .unwrap();
            let import_map_path = sub_matches.get_one::<String>("import-map").cloned();

            exit_with_code(start_server(
                &ip.as_str(),
                port,
                services_dir,
                mem_limit,
                service_timeout,
                no_module_cache,
                import_map_path,
            ))
        }
        _ => {
            // unrecognized command
        }
    }
}