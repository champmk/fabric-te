//! clap. `topo` and `run --policy naive` are live (§16.1). `plan`/`explain` stay stubs.

use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use clap::{error::ErrorKind, Parser, Subcommand};
use fabric_ctrl::{run_sim, RunConfig};
use fabric_model::{check_isolated, load_mix};
use fabric_report::write_html;
use fabric_topo::{default_rails, format_endpoint, Graph};
use fabric_types::{Policy, ProcessExit};
use sha2::{Digest, Sha256};

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
    /// Run a mix (§16.1).
    Run {
        #[arg(long)]
        topo: Option<String>,
        #[arg(long)]
        mix: Option<PathBuf>,
        #[arg(long)]
        policy: Option<String>,
        #[arg(long)]
        fail: Vec<String>,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long, default_value = "./out")]
        out: PathBuf,
        #[arg(long)]
        strict: bool,
    },
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
        Command::Run {
            topo,
            mix,
            policy,
            fail,
            seed,
            out,
            strict,
        } => cmd_run(topo, mix, policy, fail, seed, out, strict),
        Command::Plan | Command::Explain => {
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
    let write_ok = if json {
        writeln!(
            out,
            "{{\"N\":{n},\"L\":{l},\"S\":{s},\"E_host\":{e_host},\"E_ls\":{e_ls},\"B_bisect_gbps\":{b}}}"
        )
        .is_ok()
    } else if dump {
        (|| {
            writeln!(out, "N L S E_host E_ls B_bisect_gbps")?;
            writeln!(out, "{n} {l} {s} {e_host} {e_ls} {b}")?;
            writeln!(out, "link_id src dst")?;
            for link in graph.links.iter().take(16) {
                writeln!(
                    out,
                    "{} {} {}",
                    link.id.0,
                    format_endpoint(link.src),
                    format_endpoint(link.dst)
                )?;
            }
            Ok::<(), io::Error>(())
        })()
        .is_ok()
    } else {
        writeln!(out, "{n} {l} {s} {e_host} {e_ls} {b}").is_ok()
    };
    if write_ok {
        ProcessExit::Ok as i32
    } else {
        let _ = writeln!(io::stderr(), "error[E_IO]: stdout write failed");
        ProcessExit::IoAbort as i32
    }
}

fn cmd_run(
    topo: Option<String>,
    mix: Option<PathBuf>,
    policy: Option<String>,
    fail: Vec<String>,
    seed: u64,
    out: PathBuf,
    strict: bool,
) -> i32 {
    let Some(topo) = topo else {
        let _ = writeln!(io::stderr(), "error[E_USAGE]: missing --topo");
        return ProcessExit::Usage as i32;
    };
    let Some(mix_path) = mix else {
        let _ = writeln!(io::stderr(), "error[E_USAGE]: missing --mix");
        return ProcessExit::Usage as i32;
    };
    let Some(policy_s) = policy else {
        let _ = writeln!(io::stderr(), "error[E_USAGE]: missing --policy");
        return ProcessExit::Usage as i32;
    };
    let policy = match policy_s.as_str() {
        "naive" => Policy::Naive,
        "joint" => {
            let _ = writeln!(io::stderr(), "error[E_USAGE]: joint not implemented");
            return ProcessExit::Usage as i32;
        }
        _ => {
            let _ = writeln!(
                io::stderr(),
                "error[E_USAGE]: --policy must be naive or joint"
            );
            return ProcessExit::Usage as i32;
        }
    };
    for spec in &fail {
        if let Err(msg) = parse_fail_spec(spec) {
            let _ = writeln!(io::stderr(), "error[E_FAILSPEC]: {msg}");
            return ProcessExit::BadInput as i32;
        }
    }
    let out_dir = match ensure_out_dir(&out) {
        Ok(p) => p,
        Err(msg) => {
            let _ = writeln!(io::stderr(), "{msg}");
            return ProcessExit::IoAbort as i32;
        }
    };

    let (graph, topo_hash) = match load_topo(&topo) {
        Ok(v) => v,
        Err((code, msg)) => {
            let _ = writeln!(io::stderr(), "{msg}");
            return code as i32;
        }
    };
    let mix_bytes = match std::fs::read(&mix_path) {
        Ok(b) => b,
        Err(e) => {
            let _ = writeln!(io::stderr(), "error[E_IO]: {}: {e}", mix_path.display());
            return ProcessExit::IoAbort as i32;
        }
    };
    let mix_hash = sha256_hex(&mix_bytes);
    let loaded = match load_mix(&mix_path) {
        Ok(m) => m,
        Err(e) => {
            let _ = writeln!(io::stderr(), "{e}");
            return e.exit() as i32;
        }
    };
    if check_isolated(&loaded, graph.gpus.len() as u32).is_err() {
        let _ = writeln!(
            io::stderr(),
            "error[E_MIX]: isolated T_pred exceeds deadline or gpu_count > G_tot"
        );
        return ProcessExit::MixDoesNotFit as i32;
    }

    let report = match run_sim(RunConfig {
        graph,
        mix: loaded,
        policy,
        seed,
        out: out_dir.clone(),
        strict,
        mix_hash,
        topo_hash,
    }) {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(io::stderr(), "{e}");
            return e.exit() as i32;
        }
    };
    if let Err(e) = report.write_json(&out_dir.join("report.json")) {
        let _ = writeln!(io::stderr(), "error[E_IO]: report.json: {e}");
        return ProcessExit::IoAbort as i32;
    }
    if let Err(e) = write_html(&report, &out_dir.join("report.html")) {
        let _ = writeln!(io::stderr(), "error[E_IO]: report.html: {e}");
        return ProcessExit::IoAbort as i32;
    }
    if let Err(code) = report.print_stdout() {
        let _ = writeln!(io::stderr(), "error[E_IO]: stdout write failed");
        return code as i32;
    }
    ProcessExit::Ok as i32
}

fn builtin_gpus(name: &str) -> Option<u32> {
    match name {
        "n32" => Some(256),
        "n64" => Some(512),
        "n256" => Some(2048),
        "n1024" => Some(8192),
        _ => None,
    }
}

fn load_topo(spec: &str) -> Result<(Graph, String), (ProcessExit, String)> {
    if let Some(g_tot) = builtin_gpus(spec) {
        let graph = Graph::generate(g_tot, 8, 1)
            .map_err(|e| (ProcessExit::BadInput, format!("error[{}]: {e}", e.e_code())))?;
        // Builtin hash is SHA-256 of the builtin name bytes (stable; see PR6 summary).
        return Ok((graph, sha256_hex(spec.as_bytes())));
    }
    let bytes = std::fs::read(spec)
        .map_err(|e| (ProcessExit::IoAbort, format!("error[E_IO]: {spec}: {e}")))?;
    let text = String::from_utf8_lossy(&bytes);
    let graph = Graph::from_toml(&text)
        .map_err(|e| (ProcessExit::BadInput, format!("error[{}]: {e}", e.e_code())))?;
    Ok((graph, sha256_hex(&bytes)))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// `--fail` grammar §14.1. Valid specs are ignored in PR6 (no Fail* handling).
fn parse_fail_spec(s: &str) -> Result<(), String> {
    let (head, time) = match s.split_once('@') {
        Some((h, t)) => (h, Some(t)),
        None => (s, None),
    };
    let (kind, id) = head
        .split_once('=')
        .ok_or_else(|| format!("expected kind=id, got {s}"))?;
    match kind {
        "spine" | "leaf" | "rail" | "link" => {}
        _ => return Err(format!("unknown fail kind {kind}")),
    }
    if id.parse::<u32>().is_err() {
        return Err(format!("bad fail id {id}"));
    }
    if let Some(t) = time {
        parse_fail_time(t)?;
    }
    Ok(())
}

fn parse_fail_time(s: &str) -> Result<i128, String> {
    let unit_at = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
    let (num, unit) = s.split_at(unit_at);
    let x: f64 = num.parse().map_err(|_| format!("bad fail time {s}"))?;
    if !x.is_finite() {
        return Err(format!("non-finite fail time {s}"));
    }
    let secs = match unit {
        "" | "s" => x,
        "ms" => x * 1e-3,
        "us" => x * 1e-6,
        "ns" => x * 1e-9,
        "ps" => x * 1e-12,
        _ => return Err(format!("bad fail time unit {unit}")),
    };
    Ok((secs * 1e12).round_ties_even() as i128)
}

/// Canonicalize. Dest must stay under CWD. Reject `..` and symlink escape. §16.4, §24
fn ensure_out_dir(out: &Path) -> Result<PathBuf, String> {
    if out.as_os_str().is_empty() {
        return Err("error[E_IO]: --out is empty".into());
    }
    if out.starts_with("~") {
        return Err("error[E_IO]: --out must not expand ~".into());
    }
    let cwd = std::env::current_dir()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("error[E_IO]: cwd: {e}"))?;
    let joined = if out.is_absolute() {
        out.to_path_buf()
    } else {
        cwd.join(out)
    };
    let mut lex = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::Prefix(p) => lex.push(p.as_os_str()),
            Component::RootDir => lex.push(c),
            Component::CurDir => {}
            Component::ParentDir => {
                if !lex.pop() {
                    return Err("error[E_IO]: --out escapes CWD".into());
                }
            }
            Component::Normal(s) => lex.push(s),
        }
    }
    std::fs::create_dir_all(&lex).map_err(|e| format!("error[E_IO]: --out: {e}"))?;
    let canon = lex
        .canonicalize()
        .map_err(|e| format!("error[E_IO]: --out: {e}"))?;
    if !path_under(&canon, &cwd) {
        return Err("error[E_IO]: --out escapes CWD".into());
    }
    Ok(canon)
}

fn path_under(path: &Path, root: &Path) -> bool {
    let strip = |p: &Path| {
        let s = p.to_string_lossy();
        s.strip_prefix(r"\\?\").unwrap_or(&s).replace('/', "\\")
    };
    let p = strip(path);
    let r = strip(root);
    if cfg!(windows) {
        let p = p.to_ascii_lowercase();
        let r = r.to_ascii_lowercase();
        p == r || p.starts_with(&(r.clone() + "\\"))
    } else {
        p == r || p.starts_with(&(r.clone() + std::path::MAIN_SEPARATOR.to_string().as_str()))
    }
}
