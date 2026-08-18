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
