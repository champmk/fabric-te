//! Goldens: empty-cluster, default-mix-512, spine-down, moe-burst, row-late, example-c.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use fabric_ctrl::{example_c, run_sim, RunConfig};
use fabric_model::{load_mix, pairwise_alltoall_ps};
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
        .join(format!("out-pr6-{tag}-{}", std::process::id()))
}

fn run_policy(topo: &str, mix: &Path, out: &Path, seed: u64, policy: &str) -> std::process::Output {
    run_policy_fails(topo, mix, out, seed, policy, &[])
}

fn run_policy_fails(
    topo: &str,
    mix: &Path,
    out: &Path,
    seed: u64,
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
        &seed.to_string(),
        "--out",
        out.to_str().expect("out utf8"),
    ]);
    for f in fails {
        cmd.args(["--fail", f]);
    }
    cmd.output().expect("fabric-te run")
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

#[test]
fn spine_down_golden() {
    let mix = fixtures().join("mix/spine-down.toml");
    let out = out_dir("spine-down-joint");
    let got = run_policy_fails("n64", &mix, &out, 1, "joint", &["spine=3@1s"]);
    let err = String::from_utf8_lossy(&got.stderr);
    assert_eq!(got.status.code(), Some(0), "stderr={err}");
    let r = read_report(&out);
    assert_eq!(r["counts"]["admits"], 1, "admits");
    assert_eq!(r["counts"]["kills"], 0, "kills");
    assert_eq!(r["fails"][0]["epoch_to"], 1, "EpochId==1");
    assert_eq!(r["fails"][0]["dead_link_bytes"], 0, "I2");
    assert_eq!(r["fails"][0]["kills"].as_array().map(|a| a.len()), Some(0));
    let n_reroute = r["fails"][0]["reroutes"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(
        n_reroute <= 1,
        "at most the one job rerouted, got {n_reroute}"
    );
    let golden = fixtures().join("golden/spine-down/joint.report.json");
    if golden.exists() {
        let g: Value = serde_json::from_str(&fs::read_to_string(&golden).expect("golden"))
            .expect("golden json");
        assert_eq!(g["counts"]["admits"], 1);
        assert_eq!(g["counts"]["kills"], 0);
    }
    let _ = fs::remove_dir_all(&out);

    // J1 finishes ~0.26 s; @50ms is mid-run so prepare must reroute (7 spines remain).
    let out_live = out_dir("spine-down-live");
    let got_live = run_policy_fails("n64", &mix, &out_live, 1, "joint", &["spine=3@50ms"]);
    let err_live = String::from_utf8_lossy(&got_live.stderr);
    assert_eq!(got_live.status.code(), Some(0), "live stderr={err_live}");
    let rl = read_report(&out_live);
    assert_eq!(rl["counts"]["kills"], 0);
    assert_eq!(rl["fails"][0]["epoch_to"], 1);
    let live_reroutes = rl["fails"][0]["reroutes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(live_reroutes, vec![serde_json::json!(1)], "job rerouted");
    let _ = fs::remove_dir_all(&out_live);

    let out_n = out_dir("spine-down-naive");
    let got_n = run_policy_fails("n64", &mix, &out_n, 1, "naive", &["spine=3@1s"]);
    let err_n = String::from_utf8_lossy(&got_n.stderr);
    assert_eq!(got_n.status.code(), Some(0), "naive stderr={err_n}");
    let rn = read_report(&out_n);
    assert_eq!(rn["counts"]["admits"], 1, "naive admits");
    assert_eq!(rn["counts"]["kills"], 0, "naive kills");
    let golden_n = fixtures().join("golden/spine-down/naive.report.json");
    if golden_n.exists() {
        let g: Value = serde_json::from_str(&fs::read_to_string(&golden_n).expect("golden naive"))
            .expect("golden naive json");
        assert_eq!(g["counts"]["admits"], 1);
        assert_eq!(g["counts"]["kills"], 0);
    }
    let _ = fs::remove_dir_all(&out_n);
}

fn run_plan_cli(topo: &str, mix: &Path, out: &Path, deltas: &[&str]) -> std::process::Output {
    let _ = fs::remove_dir_all(out);
    let mut cmd = bin();
    cmd.args([
        "plan",
        "--topo",
        topo,
        "--mix",
        mix.to_str().expect("mix utf8"),
        "--out",
        out.to_str().expect("out utf8"),
    ]);
    for d in deltas {
        cmd.args(["--delta", d]);
    }
    cmd.output().expect("fabric-te plan")
}

fn copy_golden_if_missing(src: &Path, dest: &Path) {
    if dest.exists() {
        return;
    }
    if let Some(p) = dest.parent() {
        fs::create_dir_all(p).unwrap_or_else(|e| panic!("mkdir {}: {e}", p.display()));
    }
    fs::copy(src, dest).unwrap_or_else(|e| panic!("copy golden {}: {e}", dest.display()));
}

/// Gate field. `mean_link_util_ppm` is recorded but never a pass/fail.
fn assert_last_flow_present(r: &Value) {
    let v = r
        .get("metrics")
        .and_then(|m| m.get("last_flow_collective_us_max"))
        .expect("last_flow_collective_us_max present");
    assert!(
        v.is_number(),
        "last_flow_collective_us_max must be an integer, got {v}"
    );
}

fn admit_lines(out: &Path) -> Vec<Value> {
    let s = fs::read_to_string(out.join("admit.jsonl")).expect("admit.jsonl");
    s.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("admit jsonl"))
        .collect()
}

fn job_row<'a>(r: &'a Value, id: u32) -> &'a Value {
    r["jobs"]
        .as_array()
        .expect("jobs")
        .iter()
        .find(|j| j["job_id"] == id)
        .unwrap_or_else(|| panic!("job {id}"))
}

#[test]
fn moe_burst_golden() {
    let mix = fixtures().join("mix/moe-burst.toml");
    let t = pairwise_alltoall_ps(4, 67_108_864, 47.5e9);
    assert!(
        t < 2_000_000_000,
        "isolated T_a2a(p=4)={t} ps must be < 2 ms"
    );
    for policy in ["joint", "naive"] {
        let out = out_dir(&format!("moe-burst-{policy}"));
        let got = run_policy("n64", &mix, &out, 1, policy);
        let err = String::from_utf8_lossy(&got.stderr);
        assert_eq!(
            got.status.code(),
            Some(0),
            "moe-burst {policy} stderr={err}"
        );
        let r = read_report(&out);
        assert_eq!(r["counts"]["admits"], 4, "moe-burst {policy} admits");
        assert_eq!(r["counts"]["rejects"], 0, "moe-burst {policy} rejects");
        assert_last_flow_present(&r);
        let golden = fixtures().join(format!("golden/moe-burst/{policy}.report.json"));
        copy_golden_if_missing(&out.join("report.json"), &golden);
        let g: Value = serde_json::from_str(&fs::read_to_string(&golden).expect("golden"))
            .expect("golden json");
        assert_eq!(g["counts"]["admits"], 4);
        assert_eq!(g["counts"]["rejects"], 0);
        assert_last_flow_present(&g);
        let _ = fs::remove_dir_all(&out);
    }
}

#[test]
fn row_late_golden() {
    let mix = fixtures().join("mix/row-late.toml");
    let out = out_dir("row-late");
    let got = run_plan_cli("n256", &mix, &out, &["delay-row=B"]);
    let err = String::from_utf8_lossy(&got.stderr);
    assert_eq!(got.status.code(), Some(0), "row-late stderr={err}");
    let r = read_report(&out);
    assert_eq!(r["plan"]["gpus_removed"], 128, "gpus_removed");
    assert_eq!(r["counts"]["admits"], 10, "row-late admits");
    assert_eq!(r["counts"]["rejects"], 0, "row-late rejects");
    assert_eq!(r["plan"]["vs_baseline"]["admits"], 10, "vs_baseline.admits");
    let mut admits = 0u32;
    for rec in admit_lines(&out) {
        if rec["decision"] != "admit" {
            continue;
        }
        admits += 1;
        let map = rec["chosen"]["map"].as_array().expect("chosen.map");
        for pair in map {
            let g = pair[1].as_u64().expect("gpu id") as u32;
            let node = g / 8;
            assert!(
                !(16..32).contains(&node),
                "bound GpuId {g} node {node} ∈ [16,32)"
            );
        }
    }
    assert_eq!(admits, 10);
    let golden = fixtures().join("golden/row-late/joint.report.json");
    copy_golden_if_missing(&out.join("report.json"), &golden);
    let g: Value =
        serde_json::from_str(&fs::read_to_string(&golden).expect("golden")).expect("golden json");
    assert_eq!(g["plan"]["gpus_removed"], 128);
    assert_eq!(g["counts"]["admits"], 10);
    assert_eq!(g["counts"]["rejects"], 0);
    assert_eq!(g["plan"]["vs_baseline"]["admits"], 10);
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn example_c_golden() {
    let mix_path = fixtures().join("mix/example-c.toml");
    let mix = load_mix(&mix_path).expect("example-c mix");
    let mix_hash = format!(
        "sha256:{:x}",
        Sha256::digest(&fs::read(&mix_path).expect("mix bytes"))
    );
    let topo_hash = format!("sha256:{:x}", Sha256::digest(b"n64"));
    const D_J: i64 = 3_000_000_000;

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
            strict: false,
            mix_hash: mix_hash.clone(),
            topo_hash: topo_hash.clone(),
            fails: Vec::new(),
            occupancy: occ,
            residual: Some(residual),
        })
        .expect("example-c run");
        report
            .write_json(&out.join("report.json"))
            .expect("write report");
        let r = read_report(&out);
        let lines = admit_lines(&out);
        let j1 = lines.iter().find(|v| v["job_id"] == 1).expect("J1 admit");
        let j2 = lines.iter().find(|v| v["job_id"] == 2).expect("J2 admit");
        match policy {
            Policy::Joint => {
                assert_eq!(r["counts"]["admits"], 1, "example-c joint admits");
                assert_eq!(r["counts"]["rejects"], 1, "example-c joint rejects");
                assert_eq!(r["rejects_by_code"]["ZeroLeftover"], 1);
                assert_eq!(j1["decision"], "admit");
                assert_eq!(j1["chosen"]["kind"], "RailRotate{1}");
                assert_eq!(j2["decision"], "reject");
                assert_eq!(j2["reject"], "ZeroLeftover");
                let t = job_row(&r, 1)["t_pred_ps"].as_i64().expect("t_pred");
                assert!(t <= D_J, "J1 T_pred={t} must meet 3000 µs");
            }
            Policy::Naive => {
                assert_eq!(r["counts"]["admits"], 2, "example-c naive admits");
                assert_eq!(r["counts"]["rejects"], 0, "example-c naive rejects");
                assert_eq!(j1["decision"], "admit");
                assert_eq!(j2["decision"], "admit");
                for id in [1u32, 2] {
                    let t = job_row(&r, id)["t_pred_ps"].as_i64().expect("t_pred");
                    assert!(t > D_J, "naive J{id} T_pred={t} must SLO-miss 3000 µs");
                }
                assert_eq!(r["counts"]["slo_misses"], 2, "naive both SLO-miss");
            }
        }
        let golden = fixtures().join(format!("golden/example-c/{tag}.report.json"));
        copy_golden_if_missing(&out.join("report.json"), &golden);
        let g: Value = serde_json::from_str(&fs::read_to_string(&golden).expect("golden"))
            .expect("golden json");
        assert_eq!(g["counts"]["admits"], r["counts"]["admits"]);
        assert_eq!(g["counts"]["rejects"], r["counts"]["rejects"]);
        let _ = fs::remove_dir_all(&out);
    }
}
