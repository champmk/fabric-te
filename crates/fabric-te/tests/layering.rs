//! CI layering: parse Cargo.toml (no grep). fabric-sim ↛ fabric-te; fabric-types has no workspace deps.

use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn crate_toml(name: &str) -> toml::Value {
    let p = root().join("crates").join(name).join("Cargo.toml");
    let s = fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
    toml::from_str(&s).unwrap_or_else(|e| panic!("parse {}: {e}", p.display()))
}

fn dep_table<'a>(v: &'a toml::Value, key: &str) -> Option<&'a toml::value::Table> {
    v.get(key).and_then(|d| d.as_table())
}

#[test]
fn crate_layering() {
    let sim = crate_toml("fabric-sim");
    let sim_deps = dep_table(&sim, "dependencies").cloned().unwrap_or_default();
    assert!(
        !sim_deps.contains_key("fabric-te"),
        "fabric-sim must not depend on fabric-te"
    );

    let types = crate_toml("fabric-types");
    let types_deps = dep_table(&types, "dependencies");
    match types_deps {
        None => {}
        Some(t) => {
            assert!(
                t.is_empty(),
                "fabric-types must have no workspace deps, got {t:?}"
            );
        }
    }
    assert!(
        dep_table(&types, "dev-dependencies").is_none(),
        "fabric-types must not pull workspace crates via dev-dependencies"
    );
}
