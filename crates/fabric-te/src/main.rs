//! clap. `topo` is live (§16.1–§16.2). `run`/`plan`/`explain` stay stubs (exit 1).

use std::io::{self, Write};

use clap::{error::ErrorKind, Parser, Subcommand};
use fabric_topo::{default_rails, format_endpoint, Graph};
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
    /// Print closed-form topology.
    Topo {
        /// GPU count G_tot. Must be divisible by --rails.
        #[arg(long)]
        gpus: Option<u32>,
        /// Rails (= NICs per node). Sets G = R.
        #[arg(long, default_value_t = default_rails())]
        rails: u32,
        /// Oversubscription K_Ω ∈ {1,2,4,8,16,32}.
        #[arg(long, default_value_t = 1)]
        oversub: u32,
        /// Human tables. XOR --json.
        #[arg(long)]
        dump: bool,
        /// Machine JSON. XOR --dump.
        #[arg(long)]
        json: bool,
    },
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
        Ok(cli) => dispatch(cli),
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

fn dispatch(cli: Cli) -> i32 {
    match cli.command {
        Command::Topo {
            gpus,
            rails,
            oversub,
            dump,
            json,
        } => cmd_topo(gpus, rails, oversub, dump, json),
        Command::Run | Command::Plan | Command::Explain => {
            let _ = writeln!(io::stderr(), "error[E_USAGE]: subcommand not implemented");
            ProcessExit::Usage as i32
        }
    }
}

fn cmd_topo(gpus: Option<u32>, rails: u32, oversub: u32, dump: bool, json: bool) -> i32 {
    if dump && json {
        let _ = writeln!(
            io::stderr(),
            "error[E_USAGE]: --dump and --json are mutually exclusive"
        );
        return ProcessExit::Usage as i32;
    }
    let Some(g_tot) = gpus else {
        let _ = writeln!(io::stderr(), "error[E_USAGE]: missing --gpus");
        return ProcessExit::Usage as i32;
    };
    let graph = match Graph::generate(g_tot, rails, oversub) {
        Ok(g) => g,
        Err(e) => {
            let _ = writeln!(io::stderr(), "error[{}]: {}", e.e_code(), e);
            return ProcessExit::BadInput as i32;
        }
    };
    let n = graph.params.nodes;
    let l = graph.leaves.len();
    let s = graph.spines.len();
    let e_host = graph.e_host();
    let e_ls = graph.e_ls();
    let b = graph.b_bisect_gbps();
    let mut out = io::stdout();
    if json {
        let _ = writeln!(
            out,
            "{{\"N\":{n},\"L\":{l},\"S\":{s},\"E_host\":{e_host},\"E_ls\":{e_ls},\"B_bisect_gbps\":{b}}}"
        );
        return ProcessExit::Ok as i32;
    }
    if dump {
        let _ = writeln!(out, "N L S E_host E_ls B_bisect_gbps");
        let _ = writeln!(out, "{n} {l} {s} {e_host} {e_ls} {b}");
        let _ = writeln!(out, "link_id src dst");
        for link in graph.links.iter().take(16) {
            let _ = writeln!(
                out,
                "{} {} {}",
                link.id.0,
                format_endpoint(link.src),
                format_endpoint(link.dst)
            );
        }
        return ProcessExit::Ok as i32;
    }
    let _ = writeln!(out, "{n} {l} {s} {e_host} {e_ls} {b}");
    ProcessExit::Ok as i32
}
