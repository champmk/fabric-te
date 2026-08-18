# fabric-te status

Last updated: 2026-08-18  
Repo name: **fabric-te**  
Phase: PR1 implemented. **Next: PR2.**

**Next:** PR2 — topology generator + `topo` CLI.

## How to use this file

- Flip a PR to `done` only when its named tests pass.
- Add/remove rows when the spec’s PR plan changes. This list is allowed to drift; the spec wins if they disagree — then sync this file.
- After any session that writes code or changes the plan, update **Next**, the PR table, and **Session notes**.

## Read for next (PR2)

`docs/DESIGN.md` §7 (topology math, IDs, Example A/A2), §9.3 `Graph`/`Link`/`TopoParams` (no residual), §9.5 topo TOML, §16.1–§16.2 `topo` grammar and `--dump` columns, §30 PR2.

Do **not** implement collectives, residual, admit, or `run` in PR2.

---

## End-to-end (v1)

| Gate | Status |
| --- | --- |
| Design lock frozen (`docs/DESIGN.md`) | done |
| Persistent session files (`AGENTS.md`, this file) | done |
| Git repo exists | done |
| `topo --gpus 256 --dump` matches Example A | not started |
| `run --policy naive` + `run --policy joint` on default-mix | not started |
| `explain --job` reads `admit.jsonl` | not started |
| `run --fail spine=3@…` : dead element 0 bytes | not started |
| `plan --delta delay-row=B` same engine | not started |
| I1–I10 + `parity_log_equals_counters` on all goldens | not started |
| Stranger path: clone, `cargo test --workspace` | not started |

---

## PR checklist

Status: `done` | `in progress` | `blocked` | `not started` | `dropped`

| PR | Title | Depends | Spec | Tests (must go green) | Status |
| --- | --- | --- | --- | --- | --- |
| 1 | Workspace, types, clock, FEL | — | §9, §10.1, §30 PR1 | `clock_ps_total_order`, `fel_fires_one_event` | **done** |
| 2 | Topology generator + `topo` | 1 | §7, §16.2 | `topo_n32_closed_form`, `topo_n64_closed_form`, `topo_rail_not_tor`, `topo_one_nic_per_gpu`, `topo_bisection_n32_leaf_not_spine`, `topo_ls_full_mesh_n32` | not started |
| 3 | Collective math + mix loader | 1 | §8 | `model_ring_8x64mib`, `model_ring_16x64mib`, `model_a2a_8x64mib`, `model_ring_8x64mib_47_5`, `model_beta_is_20ps_not_20ns`, `model_us_is_ps_div_1e6`, `model_units_bytes_not_bits`, `model_phase_sum_eq_closed`, `model_p1_zero`, `odd_ring_last_hop`, isolated SLO → exit 4 | not started |
| 4 | Residual + k-shortest / ECMP | 2, 3 | §11.1–§11.3 | `joint_kshortest_k8`, `joint_cost_inverse_residual`, `naive_ecmp_tiebreak_lowest_linkid` | not started |
| 5 | Naive admit + water-fill | 4 | §11.4, §12 | `naive_scan_order_node_then_rank`, `naive_admit_gpu_count_only`, `compute_before_first_collective`, `joint_waterfill_maxmin` | not started |
| 6 | Traces, report, `run`, naive golden | 5 | §9.7–§9.9, §16 | `replay_seed_deterministic`; golden `empty-cluster`; `default-mix-512` naive report | not started |
| 7 | Bindings + `explain` skeleton | 6 | §13.1–§13.2, §13.6 | `joint_k16_bound` | not started |
| 8 | Joint evaluate + Example C | 5, 7 | §13.3–§13.5, §8.6 | `joint_reject_zero_leftover`, `joint_admit_cheapest_feasible`, `naive_may_overadmit`, `simultaneous_fifo_admit`, `scratch_not_used_by_jobs` | not started |
| 9 | Failure + 2PC | 8 | §14 | `fail_spine_reroute_or_kill`, `fail_leaf_kills_single_homed`, `fail_dead_zero_bytes`, `epoch_2pc_arc_swap`; golden `spine-down` | not started |
| 10 | Planner + deltas | 8, 9 | §15 | `planner_same_engine`, `planner_delay_row_b` | not started |
| 11 | Remaining goldens | 9, 10 | §18.4 | `moe-burst`, `row-late`, `example-c` | not started |
| 12 | Invariants + parity | 11 | §18.2–§18.3 | I1–I10 on every golden; `cli_exit_codes`; `incast_last_flow_metric`; `parity_log_equals_counters` | not started |
| 13 | As-built + README freeze | 12 | §30 PR13 | stranger path only | not started |

Water-fill is **PR5**, not PR8. Fail/2PC is **PR9**. No in-place graph mutation in any PR.

---

## Locked facts (do not rediscover)

- β = **20 ps/B** (`2e-11 s/B`). Not 20 ns/B.
- `x_us = floor(x_ps / 1_000_000)`. Not `/1000`.
- Isolated T always at **47.5 GB/s** (scratch). Ring p=2 → 1414.818 µs. A2A p=4 → 1062.614 µs.
- Example C joint admit = `RailRotate{1}`. Naive J1 and J2 both SLO-miss.
- Occupancy lives on `JobTable`, not `Arc<Graph>`.
- EventKind is **14** (includes `LeafFail`, `HorizonCut`).
- CLI: `topo` `run` `plan` `explain`. No v1 visual sim.

---

## Backlog (not v1)

Only add here if we explicitly defer. Do not implement from this list without a spec addendum.

- [ ] Read-only visual / inspector (post-PR6 traces exist)
- [ ] `inspect` TUI
- [ ] MILP flag, OCS, tree AllReduce, PXN, packet-level

---

## Session notes

| Date | What happened |
| --- | --- |
| 2026-08-17 | Design lock written, reviewed (3 rounds), frozen. |
| 2026-08-18 | Repo + `AGENTS.md` + this checklist. Next = PR1. |
| 2026-08-18 | Repo moved to `Desktop/Projects/fabric-te`. |
| 2026-08-18 | PR1 landed: workspace, types, `s_to_ps`, FEL, clap stub (exit 0/1). Next = PR2. |
| 2026-08-18 | PR body format locked: Problem / Solution / Review Order / Testing. Caveman-short. `AGENTS.md`. |
| 2026-08-18 | PR1 rev2: code-review + security-review. FEL seq, drain/CLI/s_to_ps tests; NaN clock reject. |
