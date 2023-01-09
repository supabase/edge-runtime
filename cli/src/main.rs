mod logger;

use anyhow;
use base::server::Server;
use clap::{arg, value_parser, ArgAction, Command};
use log::info;

fn cli() -> Command {
    Command::new("rex")
        .about("An API gateway for running JavaScript, TypeScript, and WASM services")
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
                .arg(arg!(-H --host <HOST> "Host address to listen on").default_value("0.0.0.0"))
                .arg(
                    arg!(-p --port <PORT> "Port to listen on")
                        .default_value("9000")
                        .value_parser(value_parser!(u16)),
                )
                .arg(arg!(--dir [DIR] "Path to services directory").default_value("services")),
        )
}

fn exit_with_code(result: Result<(), anyhow::Error>) {
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
            let host = sub_matches.get_one::<String>("host").cloned().unwrap();
            let port = sub_matches.get_one::<u16>("port").copied().unwrap();
            let services_dir = sub_matches.get_one::<String>("dir").cloned().unwrap();
            let server = Server::new(host, port, services_dir);
            exit_with_code(server.listen())
        }
        _ => {
            // unrecognized command
        }
    }
}
