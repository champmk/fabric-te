# PR3 done — collective math + mix loader

Branch: `pr/3-model` (based on PR1). `crates/fabric-te/src/main.rs` not edited.

## Problem
No closed-form T. No mix file. Isolated SLO uncheckable.

## Solution
`crates/fabric-model`: §8.2 ring/A2A + `ps_to_us`/`beta`. §9.6 loader + isolated check. `fixtures/mix/empty.toml`.

## Review Order
1. `crates/fabric-model/src/lib.rs` — formulas
2. `crates/fabric-model/src/mix.rs` — loader, sort, isolated
3. `fixtures/mix/empty.toml`
4. `Cargo.toml` / `STATUS.md`

## Testing

```
cargo fmt
cargo test -p fabric-model
cargo test --workspace
```

Both commands exit 0.

### `cargo test -p fabric-model` — 18 passed

| Test | Result |
| --- | --- |
| `model_ring_8x64mib` | ok (2_362_810_240) |
| `model_ring_16x64mib` | ok (2_546_582_400) |
| `model_a2a_8x64mib` | ok (1_181_405_120) |
| `model_ring_8x64mib_47_5` | ok (2_486_431_832 ±1) |
| `model_beta_is_20ps_not_20ns` | ok |
| `model_us_is_ps_div_1e6` | ok |
| `model_units_bytes_not_bits` | ok |
| `model_phase_sum_eq_closed` | ok |
| `model_p1_zero` | ok |
| `odd_ring_last_hop` | ok |
| `empty_mix_loads` | ok |
| `unknown_key_is_exit_2` | ok |
| `shape_mismatch_is_exit_2` | ok |
| `pattern_keeps_ids_and_sorts` | ok |
| `omitted_deadline_is_twice_isolated` | ok |
| `isolated_slo_exit_4` | ok |
| `gpu_count_over_gtot_exit_4` | ok |
| `joint_reject_slo_miss` | ok |

### `cargo test --workspace` — all passed

- fabric-model: 18
- fabric-sim: 5 (`clock_ps_total_order`, `fel_fires_one_event`, `s_to_ps_ties_to_even`, `s_to_ps_rejects_nan`, `drain_fails_at_keeps_non_fails_and_seq`)
- fabric-te exit_codes: 4 (`help_exits_0`, `version_exits_0`, `missing_subcommand_exits_1`, `stub_topo_exits_1`)
- fabric-types: 2

## Files

| Path | Role |
| --- | --- |
| `crates/fabric-model/Cargo.toml` | new crate; deps: fabric-types, serde=1.0.210, toml=0.5.11 |
| `crates/fabric-model/src/lib.rs` | closed forms + goldens |
| `crates/fabric-model/src/mix.rs` | loader + isolated check |
| `fixtures/mix/empty.toml` | `horizon_s=1`, no jobs |
| `Cargo.toml` | workspace member + path dep + serde/toml |
| `Cargo.lock` | lockfile |
| `STATUS.md` | PR3 → done |

Pinned serde/toml for rustc 1.78 (newer toml 0.8 pulled `hashbrown` edition2024).

No fabric-topo dep. No CLI/topo/residual/admit.

## API

- `ring_allreduce_ps(p, payload_bytes, b_eff_Bps) -> i128`
- `pairwise_alltoall_ps(p, payload_bytes, b_eff_Bps) -> i128`
- `beta_s_per_byte(b_eff_Bps) -> f64`
- `ps_to_us(ps) -> i128`  // floor / 1_000_000
- `load_mix(path) -> Result<Mix, MixError>`
- `check_isolated(&mix, g_tot: u32) -> Result<(), ProcessExit>`
