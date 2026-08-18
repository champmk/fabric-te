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
fn stub_topo_exits_1() {
    let out = bin().arg("topo").output().expect("run");
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E_USAGE"), "{err}");
}
