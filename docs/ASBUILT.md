# fabric-te as-built (v1)

What shipped against the v0.1 design lock in `docs/DESIGN.md`.
Week-16 freeze. Does **not** rewrite the product.

Pinned: **rustc 1.78.0**, edition 2021. `EventKind` is **14** (includes
`LeafFail`, `HorizonCut`). **β = 20 ps/B** (`2e-11 s/B`), not 20 ns/B.
Occupancy lives on `JobTable`, not `Arc<Graph>`.

## Shipped

Eight crates, one binary. CLI `topo` `run` `plan` `explain`. Goldens
under `fixtures/golden/`. I1–I10 + `parity_log_equals_counters` on the
walk. Stranger path is `README.md`.

| Gate | Shipped |
| --- | --- |
| `topo --gpus 256 --dump` | `32 8 4 256 256 51200` (Example A) |
| `run --policy naive` default-mix-512 | admits=21, rejects=0 |
| `run --policy joint` default-mix-512 | admits=21, rejects=0; hotspot worse → `NOTE.md` |
| `run --fail spine=3@1s` | dead element 0 bytes; EpochId 1 |
| `plan --delta delay-row=B` | same engine; admits=10 |
| Example C | joint `RailRotate{1}` / `ZeroLeftover`; naive over-admits |
| I1–I10 + parity | `--strict` on goldens; `cli_exit_codes` |

Non-goals still cut: no TUI / `inspect`, no packet-level, no MILP / OCS /
tree AR / PXN, no satellite / LEO / space-network content.

## Deltas vs lock

Honest. Spec text is unchanged.

| Lock | Shipped | Why |
| --- | --- | --- |
| `parquet`+`arrow` (unspecified 52-class) | **`=51.0.0`**, default features off, `features = ["arrow"]` only. Pins: `hashbrown=0.14.5`, `half=2.4.1`, `chrono=0.4.38`. Uncompressed. | 52.x pulls `hashbrown 0.17` (edition **2024**). rustc **1.78** cannot build it. |
| `topo_hash` = SHA-256 of **raw file bytes** (§5) | File `--topo` hashes file bytes. Builtin `n32`/`n64`/`n256`/`n1024` hashes **the name bytes** (`sha256("n64")`, …). | Builtins have no file. Replay identity is the name. `n64` → `sha256:2616f2173c5b9cb5292a16766e8f28ed552f6193ee97964513e673c4dffc0213`. |
| `spine-down`: `--fail spine=3@1s`, job **rerouted**, kills=0 (§18.4) | Golden `@1s` is **after** the 20-step job finishes (~**0.26 s**). `reroutes=[]`, kills=0, `epoch_to=1`, `dead_link_bytes=0`. Live reroute is the unit walk at **`spine=3@50ms`** (`reroutes=[1]`). | Isolated T≈2.83 ms + 10 ms compute × 20 steps ≈ 0.26 s. `@1s` only proves I2 + epoch, not mid-run prepare. |
| Example C naive: both jobs `T = 46.976 ms` at 2.5 GB/s leftover (§8.6) | Closed form at 2.5 GB/s is **46.976 ms + 14 µs α = 46.990 ms**. When **4 hops share** that LS, water-fill `B_eff = 2.5e9/4` → **`T_pred` ≈ 4×** (187.919 ms). Golden naive J1 = 187918819200 ps; J2 = 46990204800 ps. Both ≫ 3000 µs (SLO-miss). Joint unchanged: J1 `RailRotate{1}` 2486.432 µs, J2 `ZeroLeftover`. | Four concurrent cross-rail hops on one leftover. Claim (naive over-admits; joint does not) holds. |
| Joint must beat naive on `hotspot_us` **or** `NOTE.md` (§18.4) | **`NOTE.md`**. Joint hotspot **worse** (49_801_600 vs 47_315_040). Completions 21=21. | Joint leftover is `c_e*95/100` (I9, scratch **closed**). Naive water-fills `c_e−cir` (scratch **open**). Same 16-GPU maps; ring `dp=2` is host-only. Isolated T 1414.818 µs vs 1344.177 µs. Hotspot scales ≈1414/1344. |
| Deps include `thiserror`, `anyhow`, `tracing` (§19.3) | Not taken. Errors are `error[E_CODE]:` + `ProcessExit`. | Unused. `rand` also absent (as locked). |
| Layering via `cargo deny` / rustc `public_dependency` (§6.3, PR12) | `crate_layering` parses `Cargo.toml`. No `cargo deny` in-repo. | rustc 1.78 workspace has no lint-based gate wired. |
| `explain --link` / `--fail` live leftovers (§13.6) | **`--job` reads `admit.jsonl`.** `--link` / `--fail` still print placeholders (`-` / zeros). | Skeleton from PR7. Not filled. |
| Crate DAG: `sim → model`, `sim → trace` (§6.3) | `fabric-sim` → `types` + `topo` only. `fabric-ctrl` owns the kernel and depends on `trace` + `report`. Extra modules: `sim/{paths,waterfill}.rs`, `ctrl/{kernel,table,inv}.rs`. Tests live in `crates/fabric-te/tests/`, not `tests/cli.rs`. | Same 8 crates. Edges moved so sim stays a library. |

## Goldens (measured)

| Golden | Policy | admits / rejects / kills | notes |
| --- | --- | --- | --- |
| empty-cluster | naive | 0 / 0 / 0 | n32, horizon 1 s, no jobs |
| default-mix-512 | naive | 21 / 0 / 0 | hotspot_us=47_315_040; last_flow=1344 |
| default-mix-512 | joint | 21 / 0 / 0 | hotspot_us=49_801_600; last_flow=1414; `NOTE.md` |
| moe-burst | both | 4 / 0 / 0 | last_flow is the gate, not mean util |
| spine-down | joint `@1s` | 1 / 0 / 0 | `reroutes=[]`; live `@50ms` reroutes job 1 |
| spine-down | naive `@1s` | 1 / 0 / 0 | first-fit may SLO-miss |
| row-late | plan joint | 10 / 0 / 0 | `gpus_removed=128`; `vs_baseline.admits=10` |
| example-c | joint | 1 / 1 / 0 | reject `ZeroLeftover` |
| example-c | naive | 2 / 0 / 0 | both SLO-miss; J1 T_pred is 4× leftover |

`invariants_ok=true` on every committed `report.json`.

## Locked facts that held

- β = 20 ps/B. `x_us = floor(x_ps / 1_000_000)`.
- Isolated T always at 47.5 GB/s (scratch). Ring p=2 → 1414.818 µs. A2A p=4 → 1062.614 µs.
- Occupancy on `JobTable`. Graph is static + `Unavailable` only.
- EventKind closed set of 14. Admits/rejects/kills are traces, not FEL events.
- Naive may miss the network SLO. Joint leftover never spends scratch (I9).
- Failures: 2PC + `Arc<Graph>` swap. No in-place graph mutation.
- Planner is the same engine plus a capacity delta.
- CLI is tables. No v1 visual sim.
