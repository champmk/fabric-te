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
