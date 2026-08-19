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
