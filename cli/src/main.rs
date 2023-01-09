mod logger;

use anyhow;
use clap::{arg, ArgAction, Command};
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
        .subcommand(Command::new("start").about("Start the server"))
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
        Some(("serve", _sub_matches)) => {
            info!("This command is not implemented")
        }
        _ => {
            // unrecognized command
        }
    }
}
