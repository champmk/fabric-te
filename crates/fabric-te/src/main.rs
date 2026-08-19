//! clap. `topo`, `run`, `plan`, and `explain` are live (§16.1).

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Component, Path, PathBuf};

use clap::{error::ErrorKind, Parser, Subcommand};
use fabric_ctrl::{
    parse_delta, parse_fail_spec, run_plan, run_sim, FailSpec, PlanConfig, RunConfig,
};
use fabric_model::{check_isolated, load_mix};
use fabric_report::write_html;
use fabric_topo::{default_rails, format_endpoint, Graph};
use fabric_types::{Policy, ProcessExit};
use serde_json::Value;
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
    /// What-if plan: same engine + capacity delta. §15, §16.1
    Plan {
        #[arg(long)]
        topo: Option<String>,
        #[arg(long)]
        mix: Option<PathBuf>,
        #[arg(long)]
        delta: Vec<String>,
        #[arg(long)]
        fail: Vec<String>,
        #[arg(long, default_value = "joint")]
        policy: String,
        #[arg(long, default_value = "./out")]
        out: PathBuf,
    },
    /// Explain an admit/reject from admit.jsonl (§13.6, §16.1).
    Explain {
        #[arg(long)]
        run: Option<PathBuf>,
        #[arg(long)]
        job: Option<u32>,
        #[arg(long)]
        link: Option<u32>,
        #[arg(long)]
        fail: Option<String>,
    },
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
        Command::Plan {
            topo,
            mix,
            delta,
            fail,
            policy,
            out,
        } => cmd_plan(topo, mix, delta, fail, policy, out),
        Command::Explain {
            run,
            job,
            link,
            fail,
        } => cmd_explain(run, job, link, fail),
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
        "joint" => Policy::Joint,
        _ => {
            let _ = writeln!(
                io::stderr(),
                "error[E_USAGE]: --policy must be naive or joint"
            );
            return ProcessExit::Usage as i32;
        }
    };
    let mut fails: Vec<FailSpec> = Vec::new();
    for spec in &fail {
        match parse_fail_spec(spec) {
            Ok(f) => fails.push(f),
            Err(msg) => {
                let _ = writeln!(io::stderr(), "error[E_FAILSPEC]: {msg}");
                return ProcessExit::BadInput as i32;
            }
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
        fails,
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

fn cmd_plan(
    topo: Option<String>,
    mix: Option<PathBuf>,
    delta: Vec<String>,
    fail: Vec<String>,
    policy_s: String,
    out: PathBuf,
) -> i32 {
    let Some(topo) = topo else {
        let _ = writeln!(io::stderr(), "error[E_USAGE]: missing --topo");
        return ProcessExit::Usage as i32;
    };
    let Some(mix_path) = mix else {
        let _ = writeln!(io::stderr(), "error[E_USAGE]: missing --mix");
        return ProcessExit::Usage as i32;
    };
    let policy = match policy_s.as_str() {
        "naive" => Policy::Naive,
        "joint" => Policy::Joint,
        _ => {
            let _ = writeln!(
                io::stderr(),
                "error[E_USAGE]: --policy must be naive or joint"
            );
            return ProcessExit::Usage as i32;
        }
    };
    let mut deltas = Vec::new();
    let mut delta_specs = Vec::new();
    for spec in &delta {
        match parse_delta(spec) {
            Ok(d) => {
                deltas.push(d);
                delta_specs.push(spec.clone());
            }
            Err(msg) => {
                let _ = writeln!(io::stderr(), "error[E_FAILSPEC]: {msg}");
                return ProcessExit::BadInput as i32;
            }
        }
    }
    let mut fails: Vec<FailSpec> = Vec::new();
    for spec in &fail {
        match parse_fail_spec(spec) {
            Ok(f) => fails.push(f),
            Err(msg) => {
                let _ = writeln!(io::stderr(), "error[E_FAILSPEC]: {msg}");
                return ProcessExit::BadInput as i32;
            }
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

    let outcome = match run_plan(PlanConfig {
        graph,
        mix: loaded,
        policy,
        seed: 1,
        out: out_dir.clone(),
        strict: false,
        mix_hash,
        topo_hash,
        fails,
        deltas,
        delta_specs,
    }) {
        Ok(o) => o,
        Err(e) => {
            let _ = writeln!(io::stderr(), "{e}");
            return e.exit() as i32;
        }
    };
    let report = outcome.report;
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
    if outcome.mix_does_not_fit {
        let _ = writeln!(
            io::stderr(),
            "error[E_MIX]: isolated T_pred exceeds deadline or gpu_count > G_tot"
        );
        return ProcessExit::MixDoesNotFit as i32;
    }
    ProcessExit::Ok as i32
}

const ADMIT_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const ADMIT_MAX_ROWS: usize = 50_000_000;

fn cmd_explain(
    run: Option<PathBuf>,
    job: Option<u32>,
    link: Option<u32>,
    fail: Option<String>,
) -> i32 {
    let Some(dir) = run else {
        let _ = writeln!(io::stderr(), "error[E_USAGE]: missing --run");
        return ProcessExit::Usage as i32;
    };
    let n = usize::from(job.is_some()) + usize::from(link.is_some()) + usize::from(fail.is_some());
    if n == 0 {
        let _ = writeln!(
            io::stderr(),
            "error[E_USAGE]: missing --job, --link, or --fail"
        );
        return ProcessExit::Usage as i32;
    }
    if n > 1 {
        let _ = writeln!(
            io::stderr(),
            "error[E_USAGE]: specify one of --job, --link, --fail"
        );
        return ProcessExit::Usage as i32;
    }
    match open_admit(&dir) {
        Ok(_) => {}
        Err((code, msg)) => {
            let _ = writeln!(io::stderr(), "{msg}");
            return code as i32;
        }
    }
    let text = if let Some(j) = job {
        match load_job_record(&dir, j) {
            Ok(rec) => format_job_explain(&rec),
            Err((code, msg)) => {
                let _ = writeln!(io::stderr(), "{msg}");
                return code as i32;
            }
        }
    } else if link.is_some() {
        format_link_explain()
    } else {
        format_fail_explain()
    };
    if write!(io::stdout(), "{text}").is_ok() {
        ProcessExit::Ok as i32
    } else {
        let _ = writeln!(io::stderr(), "error[E_IO]: stdout write failed");
        ProcessExit::IoAbort as i32
    }
}

fn open_admit(dir: &Path) -> Result<File, (ProcessExit, String)> {
    let path = dir.join("admit.jsonl");
    let f = File::open(&path).map_err(|e| {
        let code = if e.kind() == io::ErrorKind::InvalidData {
            ProcessExit::BadInput
        } else {
            ProcessExit::IoAbort
        };
        let tag = if code == ProcessExit::BadInput {
            "E_PARSE"
        } else {
            "E_IO"
        };
        (code, format!("error[{tag}]: {}: {e}", path.display()))
    })?;
    let meta = f.metadata().map_err(|e| {
        (
            ProcessExit::IoAbort,
            format!("error[E_IO]: {}: {e}", path.display()),
        )
    })?;
    if meta.len() > ADMIT_MAX_BYTES {
        return Err((
            ProcessExit::IoAbort,
            format!("error[E_IO]: {}: exceeds 2 GiB", path.display()),
        ));
    }
    Ok(f)
}

fn load_job_record(dir: &Path, job: u32) -> Result<Value, (ProcessExit, String)> {
    let f = open_admit(dir)?;
    let reader = BufReader::new(f);
    let mut found = None;
    let mut rows = 0usize;
    for line in reader.lines() {
        let line = line.map_err(|e| (ProcessExit::IoAbort, format!("error[E_IO]: {e}")))?;
        if line.trim().is_empty() {
            continue;
        }
        rows += 1;
        if rows > ADMIT_MAX_ROWS {
            return Err((
                ProcessExit::IoAbort,
                "error[E_IO]: admit.jsonl exceeds 50e6 rows".into(),
            ));
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if job_id_of(&v) == Some(job) {
            found = Some(v);
        }
    }
    found.ok_or_else(|| {
        (
            ProcessExit::BadInput,
            format!("error[E_PARSE]: job {job} not in admit.jsonl"),
        )
    })
}

fn job_id_of(v: &Value) -> Option<u32> {
    match v.get("job_id") {
        Some(Value::Number(n)) => n.as_u64().and_then(|x| u32::try_from(x).ok()),
        Some(Value::String(s)) => s.parse().ok(),
        _ => None,
    }
}

fn format_job_explain(rec: &Value) -> String {
    let mut out = String::new();
    push_kv(&mut out, "job_id", scalar(rec.get("job_id")));
    push_kv(&mut out, "policy", scalar(rec.get("policy")));
    push_kv(&mut out, "decision", scalar(rec.get("decision")));
    push_kv(&mut out, "reject", scalar(rec.get("reject")));
    push_kv(
        &mut out,
        "free_at_arrive",
        scalar(rec.get("free_at_arrive")),
    );
    push_kv(
        &mut out,
        "bindings_evaluated",
        scalar(rec.get("bindings_evaluated")),
    );
    push_list(&mut out, "per_binding[]", rec.get("per_binding"));
    push_obj(&mut out, "chosen", rec.get("chosen"));
    push_list(&mut out, "per_link[]", rec.get("per_link"));
    push_list(&mut out, "waterfill[]", rec.get("waterfill"));
    push_kv(&mut out, "B_eff_Bps", scalar(rec.get("B_eff_Bps")));
    push_kv(&mut out, "T_pred_ps", scalar(rec.get("T_pred_ps")));
    push_kv(&mut out, "D_j_ps", scalar(rec.get("D_j_ps")));
    push_obj(&mut out, "naive_compare", rec.get("naive_compare"));
    out
}

fn format_link_explain() -> String {
    // Placeholders until PR8/PR9 traces carry live leftover. §13.6
    let mut out = String::new();
    for k in [
        "c",
        "scratch",
        "cir",
        "r_avail",
        "failed",
        "flows now",
        "hotspot_us",
    ] {
        push_kv(&mut out, k, "-");
    }
    out
}

fn format_fail_explain() -> String {
    // Placeholders until PR9 2PC. §13.6
    let mut out = String::new();
    push_kv(&mut out, "epoch", "0");
    push_kv(&mut out, "jobs rerouted", "0");
    push_kv(&mut out, "jobs killed", "0");
    push_kv(&mut out, "T_pred before/after", "-");
    out
}

fn push_kv(out: &mut String, k: &str, v: impl AsRef<str>) {
    out.push_str(k);
    out.push_str(": ");
    out.push_str(v.as_ref());
    out.push('\n');
}

fn push_list(out: &mut String, k: &str, v: Option<&Value>) {
    out.push_str(k);
    out.push_str(":\n");
    out.push_str(&indent_block(&pretty_or_empty(v)));
    out.push('\n');
}

fn push_obj(out: &mut String, k: &str, v: Option<&Value>) {
    out.push_str(k);
    out.push_str(":\n");
    match v {
        None | Some(Value::Null) => {
            out.push_str("  -\n");
        }
        Some(val) => {
            out.push_str(&indent_block(&pretty_value(val)));
            out.push('\n');
        }
    }
}

fn scalar(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "-".into(),
        Some(Value::String(s)) if s.is_empty() => "-".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => "-".into(),
    }
}

fn pretty_or_empty(v: Option<&Value>) -> String {
    match v {
        Some(a @ Value::Array(_)) => pretty_value(a),
        _ => "[]".into(),
    }
}

fn pretty_value(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| "-".into())
}

fn indent_block(s: &str) -> String {
    let mut out = String::new();
    for (i, line) in s.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str("  ");
        out.push_str(line);
    }
    out
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
