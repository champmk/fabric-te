//! clap stub. Exit 0 on --help/--version; exit 1 on usage. No topo/run yet.

use clap::{error::ErrorKind, Parser, Subcommand};
use fabric_types::ProcessExit;

#[derive(Parser, Debug)]
#[command(
    name = "fabric-te",
    version,
    about = "Joint placement and path admission for a simulated GPU fabric."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print closed-form topology (PR2).
    Topo,
    /// Run a mix (PR6).
    Run,
    /// What-if plan (PR10).
    Plan,
    /// Explain an admit/reject (PR7).
    Explain,
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    match Cli::try_parse() {
        Ok(_cli) => ProcessExit::Ok as i32,
        Err(e) if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) => {
            let _ = e.print();
            ProcessExit::Ok as i32
        }
        Err(e) => {
            let _ = e.print();
            ProcessExit::Usage as i32
        }
    }
}
