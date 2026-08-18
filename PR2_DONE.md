# PR2 done

Topology generator + `topo` CLI. Transcribed from `docs/DESIGN.md` §7, §9.3, §9.5, §16.1–§16.2. No Residual.

## Files changed

- `Cargo.toml` — workspace member `fabric-topo`; serde/toml pins (rustc 1.78)
- `Cargo.lock`
- `STATUS.md` — PR2 done, next PR3
- `crates/fabric-topo/Cargo.toml` — new crate (fabric-types + serde/toml)
- `crates/fabric-topo/src/lib.rs` — Graph/Node/Gpu/Leaf/Spine/Link/TopoParams, §7.2–§7.5 build, TOML load
- `crates/fabric-te/Cargo.toml` — path dep `fabric-topo`
- `crates/fabric-te/src/main.rs` — `topo --gpus/--rails/--oversub [--dump|--json]`
- `crates/fabric-te/tests/exit_codes.rs` — topo without `--gpus` → 1; `--gpus 256` → 0; dump XOR json → 1
- `fixtures/topo/n32.toml` — §9.5

## `cargo test --workspace`

Exit 0. 22 passed, 0 failed.

| Crate / suite | Result |
| --- | --- |
| fabric-sim | 5 passed (`clock_ps_total_order`, `fel_fires_one_event`, …) |
| fabric-types | 2 passed |
| fabric-topo | 8 passed |
| fabric-te `exit_codes` | 7 passed |
| doc-tests | 0 |

Named PR2 tests: `topo_n32_closed_form`, `topo_n64_closed_form`, `topo_rail_not_tor`, `topo_one_nic_per_gpu`, `topo_bisection_n32_leaf_not_spine`, `topo_ls_full_mesh_n32` — all ok.

CLI check: `fabric-te topo --gpus 256` prints `32 8 4 256 256 51200`. `--dump` header + values + first 16 host links. `--dump --json` exit 1.
