use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_fabric-te"))
}

#[test]
fn help_exits_0() {
    let out = bin().arg("--help").output().expect("run");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn version_exits_0() {
    let out = bin().arg("--version").output().expect("run");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn missing_subcommand_exits_1() {
    let out = bin().output().expect("run");
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn topo_without_gpus_exits_1() {
    let out = bin().arg("topo").output().expect("run");
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E_USAGE"), "{err}");
}

#[test]
fn topo_gpus_256_exits_0() {
    let out = bin().args(["topo", "--gpus", "256"]).output().expect("run");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim(), "32 8 4 256 256 51200");
}

#[test]
fn topo_dump_and_json_exits_1() {
    let out = bin()
        .args(["topo", "--gpus", "256", "--dump", "--json"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E_USAGE"), "{err}");
}

#[test]
fn stub_run_exits_1() {
    let out = bin().arg("run").output().expect("run");
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E_USAGE"), "{err}");
}

#[test]
fn isolated_miss_via_mix_loader_exits_4() {
    use std::fs;
    let dir = std::env::current_dir()
        .expect("cwd")
        .join(format!("out-pr6-iso-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let mix = dir.join("bad.toml");
    fs::write(
        &mix,
        r#"
horizon_s = 1
[[jobs]]
id = 1
arrive_s = 0.0
gpu_count = 8
dp = 8
tp = 1
pp = 1
collective = "ring_allreduce"
payload_bytes = 67108864
deadline_s = 0.000001
"#,
    )
    .expect("write mix");
    let out = dir.join("out");
    let got = bin()
        .args([
            "run",
            "--topo",
            "n32",
            "--mix",
            mix.to_str().unwrap(),
            "--policy",
            "naive",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&got.stderr);
    assert_eq!(got.status.code(), Some(4), "{err}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn bad_fail_spec_exits_2() {
    let mix =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mix/empty.toml");
    let out = std::env::current_dir()
        .expect("cwd")
        .join(format!("out-pr6-fail-{}", std::process::id()));
    let got = bin()
        .args([
            "run",
            "--topo",
            "n32",
            "--mix",
            mix.to_str().unwrap(),
            "--policy",
            "naive",
            "--fail",
            "nope",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&got.stderr);
    assert_eq!(got.status.code(), Some(2), "{err}");
    assert!(err.contains("E_FAILSPEC"), "{err}");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn explain_missing_run_exits_1() {
    let out = bin().args(["explain", "--job", "1"]).output().expect("run");
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E_USAGE"), "{err}");
}

#[test]
fn explain_missing_selector_exits_1() {
    let out = bin().args(["explain", "--run", "."]).output().expect("run");
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E_USAGE"), "{err}");
}

#[test]
fn explain_missing_admit_exits_5() {
    let dir = std::env::current_dir()
        .expect("cwd")
        .join(format!("out-pr7-missing-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let got = bin()
        .args(["explain", "--run", dir.to_str().unwrap(), "--job", "1"])
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&got.stderr);
    assert_eq!(got.status.code(), Some(5), "{err}");
    assert!(err.contains("E_IO"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn explain_sparse_admit_no_panic() {
    let dir = std::env::current_dir()
        .expect("cwd")
        .join(format!("out-pr7-sparse-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(dir.join("admit.jsonl"), "{\"job_id\":1}\n").expect("write");
    let got = bin()
        .args(["explain", "--run", dir.to_str().unwrap(), "--job", "1"])
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&got.stderr);
    let stdout = String::from_utf8_lossy(&got.stdout);
    assert_eq!(got.status.code(), Some(0), "{err}");
    let keys = [
        "job_id:",
        "policy:",
        "decision:",
        "reject:",
        "free_at_arrive:",
        "bindings_evaluated:",
        "per_binding[]:",
        "chosen:",
        "per_link[]:",
        "waterfill[]:",
        "B_eff_Bps:",
        "T_pred_ps:",
        "D_j_ps:",
        "naive_compare:",
    ];
    let mut pos = 0;
    for k in keys {
        let found = stdout[pos..]
            .find(k)
            .unwrap_or_else(|| panic!("missing {k} after {pos} in:\n{stdout}"));
        pos += found + k.len();
    }
    assert!(stdout.contains("job_id: 1"), "{stdout}");
    assert!(stdout.contains("policy: -"), "{stdout}");
    assert!(stdout.contains("per_binding[]:"), "{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cli_exit_codes() {
    use fabric_ctrl::{run_sim, Occupancy, RunConfig};
    use fabric_model::Mix;
    use fabric_topo::Graph;
    use fabric_types::{GpuId, JobId, Policy, ProcessExit};

    // usage → 1
    let usage = bin().output().expect("usage");
    assert_eq!(usage.status.code(), Some(1));

    let dir = std::env::current_dir()
        .expect("cwd")
        .join(format!("out-pr12-cli-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);

    // bad toml → 2
    let bad = dir.join("bad.toml");
    std::fs::write(&bad, "this is not {{{{ toml").expect("write bad toml");
    let out2 = dir.join("out-bad");
    let got2 = bin()
        .args([
            "run",
            "--topo",
            "n32",
            "--mix",
            bad.to_str().unwrap(),
            "--policy",
            "naive",
            "--out",
            out2.to_str().unwrap(),
        ])
        .output()
        .expect("bad toml");
    let err2 = String::from_utf8_lossy(&got2.stderr);
    assert_eq!(got2.status.code(), Some(2), "{err2}");
    assert!(
        err2.contains("E_PARSE") || err2.contains("E_SCHEMA"),
        "{err2}"
    );

    // --strict broken I → 3 (same kernel the CLI calls)
    let graph = Graph::generate(256, 8, 1).expect("n32");
    let mut occ = Occupancy::new();
    occ.by_gpu.insert(GpuId(999_999), JobId(1));
    let mix = Mix {
        seed: 1,
        horizon_ps: 1_000_000_000,
        jobs: Vec::new(),
    };
    let out3 = dir.join("out-inv");
    let err = run_sim(RunConfig {
        graph,
        mix,
        policy: Policy::Naive,
        seed: 1,
        out: out3,
        strict: true,
        mix_hash: "t".into(),
        topo_hash: "t".into(),
        fails: Vec::new(),
        occupancy: occ,
        residual: None,
    })
    .expect_err("I4 must trip");
    assert_eq!(err.exit(), ProcessExit::InvariantFail);
    assert_eq!(err.exit() as i32, 3);
    let msg = err.to_string();
    assert!(msg.contains("E_INV"), "{msg}");
    assert!(msg.contains("I4"), "{msg}");

    // isolated miss → 4 (named test already covers; pin here)
    let iso = dir.join("iso.toml");
    std::fs::write(
        &iso,
        r#"
horizon_s = 1
[[jobs]]
id = 1
arrive_s = 0.0
gpu_count = 8
dp = 8
tp = 1
pp = 1
collective = "ring_allreduce"
payload_bytes = 67108864
deadline_s = 0.000001
"#,
    )
    .expect("iso mix");
    let out4 = dir.join("out-iso");
    let got4 = bin()
        .args([
            "run",
            "--topo",
            "n32",
            "--mix",
            iso.to_str().unwrap(),
            "--policy",
            "naive",
            "--out",
            out4.to_str().unwrap(),
        ])
        .output()
        .expect("iso");
    let err4 = String::from_utf8_lossy(&got4.stderr);
    assert_eq!(got4.status.code(), Some(4), "{err4}");

    // unwritable --out → 5
    let blocked = dir.join("not-a-dir");
    std::fs::write(&blocked, b"file").expect("blocker");
    let mix_ok =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mix/empty.toml");
    let got5 = bin()
        .args([
            "run",
            "--topo",
            "n32",
            "--mix",
            mix_ok.to_str().unwrap(),
            "--policy",
            "naive",
            "--out",
            blocked.to_str().unwrap(),
            "--strict",
        ])
        .output()
        .expect("unwritable");
    let err5 = String::from_utf8_lossy(&got5.stderr);
    assert_eq!(got5.status.code(), Some(5), "{err5}");
    assert!(err5.contains("E_IO"), "{err5}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn explain_link_and_fail_placeholders() {
    let dir = std::env::current_dir()
        .expect("cwd")
        .join(format!("out-pr7-ph-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(dir.join("admit.jsonl"), "").expect("write");
    let link = bin()
        .args(["explain", "--run", dir.to_str().unwrap(), "--link", "0"])
        .output()
        .expect("run");
    assert_eq!(link.status.code(), Some(0));
    let ls = String::from_utf8_lossy(&link.stdout);
    for k in [
        "c:",
        "scratch:",
        "cir:",
        "r_avail:",
        "failed:",
        "flows now:",
        "hotspot_us:",
    ] {
        assert!(ls.contains(k), "missing {k} in:\n{ls}");
    }
    let fail = bin()
        .args([
            "explain",
            "--run",
            dir.to_str().unwrap(),
            "--fail",
            "spine=3",
        ])
        .output()
        .expect("run");
    assert_eq!(fail.status.code(), Some(0));
    let fs = String::from_utf8_lossy(&fail.stdout);
    assert!(fs.contains("epoch:"), "{fs}");
    assert!(fs.contains("jobs rerouted:"), "{fs}");
    assert!(fs.contains("jobs killed:"), "{fs}");
    assert!(fs.contains("T_pred before/after:"), "{fs}");
    let _ = std::fs::remove_dir_all(&dir);
}
