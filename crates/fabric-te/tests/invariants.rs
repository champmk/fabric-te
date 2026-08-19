//! PR12: I1–I10 on every golden, I6 parity, incast last-flow gate.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use fabric_ctrl::{example_c, run_plan, run_sim, Delta, PlanConfig, RunConfig};
use fabric_model::load_mix;
use fabric_topo::Graph;
use fabric_trace::rollup_dir;
use fabric_types::Policy;
use serde_json::Value;
use sha2::{Digest, Sha256};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_fabric-te"))
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn out_dir(tag: &str) -> PathBuf {
    std::env::current_dir()
        .expect("cwd")
        .join(format!("out-pr12-{tag}-{}", std::process::id()))
}

fn read_report(out: &Path) -> Value {
    let p = out.join("report.json");
    let s = fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("parse report: {e}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn run_strict(
    topo: &str,
    mix: &Path,
    out: &Path,
    policy: &str,
    fails: &[&str],
) -> std::process::Output {
    let _ = fs::remove_dir_all(out);
    let mut cmd = bin();
    cmd.args([
        "run",
        "--topo",
        topo,
        "--mix",
        mix.to_str().expect("mix utf8"),
        "--policy",
        policy,
        "--seed",
        "1",
        "--out",
        out.to_str().expect("out utf8"),
        "--strict",
    ]);
    for f in fails {
        cmd.args(["--fail", f]);
    }
    cmd.output().expect("fabric-te run --strict")
}

fn assert_invariants_ok(r: &Value, name: &str) {
    assert_eq!(
        r.get("invariants_ok").and_then(|v| v.as_bool()),
        Some(true),
        "{name} invariants_ok"
    );
}

/// Goldens gate on last_flow_collective_us_max. mean_link_util_ppm is recorded, never a gate.
fn assert_last_flow_present(r: &Value, name: &str) {
    let v = r
        .get("metrics")
        .and_then(|m| m.get("last_flow_collective_us_max"))
        .unwrap_or_else(|| panic!("{name}: last_flow_collective_us_max present"));
    assert!(
        v.is_number(),
        "{name}: last_flow_collective_us_max must be an integer, got {v}"
    );
    let _ = r.get("metrics").and_then(|m| m.get("mean_link_util_ppm"));
}

fn committed_goldens() -> Vec<(String, PathBuf)> {
    let root = fixtures().join("golden");
    let names = [
        "empty-cluster/naive.report.json",
        "default-mix-512/naive.report.json",
        "default-mix-512/joint.report.json",
        "moe-burst/naive.report.json",
        "moe-burst/joint.report.json",
        "spine-down/naive.report.json",
        "spine-down/joint.report.json",
        "row-late/joint.report.json",
        "example-c/naive.report.json",
        "example-c/joint.report.json",
    ];
    names
        .into_iter()
        .map(|n| (n.to_string(), root.join(n)))
        .collect()
}

#[test]
fn invariants_on_every_golden() {
    for (name, path) in committed_goldens() {
        let s =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let r: Value = serde_json::from_str(&s).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_invariants_ok(&r, &name);
    }

    let mix_empty = fixtures().join("mix/empty.toml");
    let mix_default = fixtures().join("mix/default-512.toml");
    let mix_moe = fixtures().join("mix/moe-burst.toml");
    let mix_spine = fixtures().join("mix/spine-down.toml");
    let mix_row = fixtures().join("mix/row-late.toml");
    let mix_c = fixtures().join("mix/example-c.toml");

    let cases: &[(&str, &str, &Path, &str, &[&str])] = &[
        ("empty-cluster", "n32", &mix_empty, "naive", &[]),
        ("default-mix-512-naive", "n64", &mix_default, "naive", &[]),
        ("default-mix-512-joint", "n64", &mix_default, "joint", &[]),
        ("moe-burst-naive", "n64", &mix_moe, "naive", &[]),
        ("moe-burst-joint", "n64", &mix_moe, "joint", &[]),
        (
            "spine-down-naive",
            "n64",
            &mix_spine,
            "naive",
            &["spine=3@1s"],
        ),
        (
            "spine-down-joint",
            "n64",
            &mix_spine,
            "joint",
            &["spine=3@1s"],
        ),
    ];
    for &(name, topo, mix, policy, fails) in cases {
        let out = out_dir(name);
        let got = run_strict(topo, mix, &out, policy, fails);
        let err = String::from_utf8_lossy(&got.stderr);
        assert_eq!(got.status.code(), Some(0), "{name} stderr={err}");
        let r = read_report(&out);
        assert_invariants_ok(&r, name);
        let _ = fs::remove_dir_all(&out);
    }

    let out_row = out_dir("row-late");
    let _ = fs::remove_dir_all(&out_row);
    let mix = load_mix(&mix_row).expect("row-late mix");
    let graph = Graph::generate(2048, 8, 1).expect("n256");
    let mix_hash = sha256_hex(&fs::read(&mix_row).expect("mix bytes"));
    let plan = run_plan(PlanConfig {
        graph,
        mix,
        policy: Policy::Joint,
        seed: 1,
        out: out_row.clone(),
        strict: true,
        mix_hash,
        topo_hash: sha256_hex(b"n256"),
        fails: Vec::new(),
        deltas: vec![Delta::DelayRow(1)],
        delta_specs: vec!["delay-row=B".into()],
    })
    .expect("row-late plan --strict");
    assert!(plan.report.invariants_ok, "row-late invariants_ok");
    let _ = fs::remove_dir_all(&out_row);

    let mix = load_mix(&mix_c).expect("example-c mix");
    let mix_hash = sha256_hex(&fs::read(&mix_c).expect("mix bytes"));
    let topo_hash = sha256_hex(b"n64");
    for policy in [Policy::Joint, Policy::Naive] {
        let (graph, residual, occ) = example_c();
        let tag = policy.as_str();
        let out = out_dir(&format!("example-c-{tag}"));
        let _ = fs::remove_dir_all(&out);
        let report = run_sim(RunConfig {
            graph,
            mix: mix.clone(),
            policy,
            seed: 1,
            out: out.clone(),
            strict: true,
            mix_hash: mix_hash.clone(),
            topo_hash: topo_hash.clone(),
            fails: Vec::new(),
            occupancy: occ,
            residual: Some(residual),
        })
        .unwrap_or_else(|e| panic!("example-c {tag} --strict: {e}"));
        assert!(report.invariants_ok, "example-c {tag} invariants_ok");
        let _ = fs::remove_dir_all(&out);
    }
}

#[test]
fn incast_last_flow_metric() {
    for (name, path) in committed_goldens() {
        let s =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let r: Value = serde_json::from_str(&s).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_last_flow_present(&r, &name);
    }
}

#[test]
fn parity_log_equals_counters() {
    let mix = fixtures().join("mix/empty.toml");
    let out = out_dir("parity-empty");
    let got = run_strict("n32", &mix, &out, "naive", &[]);
    let err = String::from_utf8_lossy(&got.stderr);
    assert_eq!(got.status.code(), Some(0), "empty stderr={err}");
    let r = read_report(&out);
    let log = rollup_dir(&out).expect("rollup empty");
    assert_eq!(log.arrivals, r["counts"]["arrivals"].as_u64().unwrap());
    assert_eq!(log.admits, r["counts"]["admits"].as_u64().unwrap());
    assert_eq!(log.rejects, r["counts"]["rejects"].as_u64().unwrap());
    assert_eq!(log.kills, r["counts"]["kills"].as_u64().unwrap());
    assert_eq!(log.completes, r["counts"]["completes"].as_u64().unwrap());
    assert_invariants_ok(&r, "parity-empty");
    let _ = fs::remove_dir_all(&out);

    let mix = fixtures().join("mix/default-512.toml");
    let out = out_dir("parity-default");
    let got = run_strict("n64", &mix, &out, "naive", &[]);
    let err = String::from_utf8_lossy(&got.stderr);
    assert_eq!(got.status.code(), Some(0), "default stderr={err}");
    let r = read_report(&out);
    let log = rollup_dir(&out).expect("rollup default");
    assert_eq!(
        log.arrivals,
        r["counts"]["arrivals"].as_u64().unwrap(),
        "arrivals"
    );
    assert_eq!(
        log.admits,
        r["counts"]["admits"].as_u64().unwrap(),
        "admits"
    );
    assert_eq!(
        log.rejects,
        r["counts"]["rejects"].as_u64().unwrap(),
        "rejects"
    );
    assert_eq!(log.kills, r["counts"]["kills"].as_u64().unwrap(), "kills");
    assert_eq!(
        log.completes,
        r["counts"]["completes"].as_u64().unwrap(),
        "completes"
    );
    assert_invariants_ok(&r, "parity-default");
    let _ = fs::remove_dir_all(&out);
}
