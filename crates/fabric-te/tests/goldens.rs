//! PR6 goldens: empty-cluster, default-mix-512 naive, replay determinism.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_fabric-te"))
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn out_dir(tag: &str) -> PathBuf {
    std::env::current_dir()
        .expect("cwd")
        .join(format!("out-pr6-{tag}-{}", std::process::id()))
}

fn run_policy(topo: &str, mix: &Path, out: &Path, seed: u64, policy: &str) -> std::process::Output {
    let _ = fs::remove_dir_all(out);
    bin()
        .args([
            "run",
            "--topo",
            topo,
            "--mix",
            mix.to_str().expect("mix utf8"),
            "--policy",
            policy,
            "--seed",
            &seed.to_string(),
            "--out",
            out.to_str().expect("out utf8"),
        ])
        .output()
        .expect("fabric-te run")
}

fn run_naive(topo: &str, mix: &Path, out: &Path, seed: u64) -> std::process::Output {
    run_policy(topo, mix, out, seed, "naive")
}

fn read_report(out: &Path) -> Value {
    let p = out.join("report.json");
    let s = fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("parse report: {e}"))
}

fn sha256_file(p: &Path) -> String {
    use sha2::{Digest, Sha256};
    let b = fs::read(p).unwrap_or_else(|e| panic!("hash {}: {e}", p.display()));
    format!("{:x}", Sha256::digest(&b))
}

#[test]
fn empty_cluster_golden() {
    let mix = fixtures().join("mix/empty.toml");
    let out = out_dir("empty");
    let got = run_naive("n32", &mix, &out, 1);
    let err = String::from_utf8_lossy(&got.stderr);
    assert_eq!(got.status.code(), Some(0), "stderr={err}");
    let r = read_report(&out);
    assert_eq!(r["topo"]["L"], 8);
    assert_eq!(r["topo"]["S"], 4);
    assert_eq!(r["topo"]["E_host"], 256);
    assert_eq!(r["topo"]["E_ls"], 256);
    assert_eq!(r["topo"]["B_bisect_gbps"], 51200);
    assert_eq!(r["counts"]["arrivals"], 0);
    assert_eq!(r["counts"]["admits"], 0);
    let meta = fs::read_to_string(out.join("meta.toml")).expect("meta");
    assert!(meta.contains("seed = 1"), "{meta}");
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn default_mix_512_naive() {
    let mix = fixtures().join("mix/default-512.toml");
    let out = out_dir("default");
    let got = run_naive("n64", &mix, &out, 1);
    let err = String::from_utf8_lossy(&got.stderr);
    assert_eq!(got.status.code(), Some(0), "stderr={err}");
    let r = read_report(&out);
    assert_eq!(r["counts"]["admits"], 21, "admits");
    assert_eq!(r["counts"]["rejects"], 0, "rejects");
    let golden = fixtures().join("golden/default-mix-512/naive.report.json");
    if golden.exists() {
        let g: Value = serde_json::from_str(&fs::read_to_string(&golden).expect("golden"))
            .expect("golden json");
        assert_eq!(g["counts"]["admits"], 21);
        assert_eq!(g["counts"]["rejects"], 0);
    }
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn default_mix_512_joint() {
    let mix = fixtures().join("mix/default-512.toml");
    let out = out_dir("default-joint");
    let got = run_policy("n64", &mix, &out, 1, "joint");
    let err = String::from_utf8_lossy(&got.stderr);
    assert_eq!(got.status.code(), Some(0), "stderr={err}");
    let r = read_report(&out);
    assert_eq!(r["counts"]["admits"], 21, "admits");
    assert_eq!(r["counts"]["rejects"], 0, "rejects");
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn replay_seed_deterministic() {
    let mix = fixtures().join("mix/empty.toml");
    let a = out_dir("replay-a");
    let b = out_dir("replay-b");
    let oa = run_naive("n32", &mix, &a, 1);
    let ob = run_naive("n32", &mix, &b, 1);
    assert_eq!(oa.status.code(), Some(0));
    assert_eq!(ob.status.code(), Some(0));
    let ra = fs::read(a.join("report.json")).expect("a report");
    let rb = fs::read(b.join("report.json")).expect("b report");
    assert_eq!(ra, rb, "report.json bytes");
    for name in [
        "events.parquet",
        "flows.parquet",
        "links.parquet",
        "jobs.parquet",
    ] {
        assert_eq!(
            sha256_file(&a.join(name)),
            sha256_file(&b.join(name)),
            "{name} hash"
        );
    }
    let _ = fs::remove_dir_all(&a);
    let _ = fs::remove_dir_all(&b);
}
