# fabric-te

Training jobs miss their deadline on a cluster that still has free GPUs,
because the cables those jobs need are already full. This repo is a
laptop-scale, deterministic flow-level simulator of that network, plus two
controllers: **naive** packs free GPUs; **joint** looks at leftover
bandwidth first. Naive is allowed to admit jobs that then miss. Joint is
not.

One CLI (`topo` `run` `plan` `explain`). Tables, not a canvas. The
operator artifact is `report.html` / `report.json`. A read-only inspector
of three locked examples lives in [`viz/index.html`](viz/index.html) — it
is not a second simulator.

Spec lock: [`docs/DESIGN.md`](docs/DESIGN.md).  
What shipped vs that lock: [`docs/ASBUILT.md`](docs/ASBUILT.md).

Requires **rustc 1.78** (`rust-toolchain.toml`).

## Stranger path

```
git clone <this-repo>
cd fabric-te
cargo test --workspace
cargo run -- topo --gpus 256 --dump
cargo run -- run --topo n64 --mix fixtures/mix/default-512.toml --policy naive
```

`topo --gpus 256 --dump` (Example A, 256 GPUs → N=32):

```
N L S E_host E_ls B_bisect_gbps
32 8 4 256 256 51200
```

then a 16-row `link_id src dst` pin. Without `--dump` / `--json`, stdout is the one closed-form line only.

`run … --policy naive` on default-mix-512: **admits=21, rejects=0**. Stdout is one header row of `counts` then `rejects_by_code` keys, then one integer row. Artifacts land in `./out/` (`report.json`, `report.html`, parquet, `admit.jsonl`).

Same mix, `--policy joint`: also admits=21, rejects=0. Joint `hotspot_us` is **worse** than naive — see `fixtures/golden/default-mix-512/NOTE.md`.

## CLI

```
fabric-te topo    --gpus G_TOT [--rails R] [--oversub K_OMEGA] [--dump|--json]
fabric-te run     --topo T --mix M --policy naive|joint
                  [--fail SPEC]... [--seed S] [--out DIR] [--strict]
fabric-te plan    --topo T --mix M --delta SPEC [--delta SPEC]...
                  [--fail SPEC]... [--policy joint] [--out DIR]
fabric-te explain --run DIR --job J
fabric-te explain --run DIR --link L
fabric-te explain --run DIR --fail spine=3
```

| Token | Rule |
| --- | --- |
| `--gpus G_TOT` | GPU count. Must be divisible by `--rails` (default 8) |
| `--oversub K_OMEGA` | ∈ {1,2,4,8,16,32}. Not the binding cap K |
| `--topo T` | Path to TOML **or** builtin `n32` `n64` `n256` `n1024` |
| `--mix M` | Path to mix TOML |
| `--policy` | Required for `run`. Default `joint` for `plan` |
| `--out DIR` | Default `./out`. Overwrite. Must stay under CWD |
| `--seed S` | u64, default 1 |
| `--strict` | Any invariant fail → exit 3 immediately |
| `--dump` / `--json` | XOR. Both → exit 1. Neither on `topo` → one-line closed form |
| `--fail` | Repeatable on `run` and `plan`. `kind=id[@t]` |
| `--delta` | Repeatable. e.g. `delay-row=B` |

`--help` / `--version` exit 0. Errors: stderr, `error[E_CODE]: message`. No color.

| Exit | When |
| --- | --- |
| 0 | ok (`plan` may still list rejects) |
| 1 | usage (bad / missing flags) |
| 2 | bad input (TOML, `gpus % rails != 0`, bad `--fail` / `--delta`) |
| 3 | invariant fail (I1–I10 or `--strict`) |
| 4 | mix does not fit (isolated T > D_j, or `p > G_tot`) |
| 5 | IO abort (`--out` unwritable, parquet flush) |

## Goldens

Under `fixtures/golden/`. `cargo test --workspace` is the suite.

| Golden | Topo | Check |
| --- | --- | --- |
| `empty-cluster` | `n32` | arrivals=0 |
| `default-mix-512` | `n64` | naive+joint admits=21 rejects=0; joint hotspot worse → `NOTE.md` |
| `moe-burst` | `n64` | admits=4 |
| `spine-down` | `n64` | `--fail spine=3@1s`; dead element 0 bytes |
| `row-late` | `n256` | `plan --delta delay-row=B`; admits=10 |
| `example-c` | `n64` | joint admits J1 `RailRotate{1}`, rejects J2 `ZeroLeftover`; naive over-admits |

v1 is complete. No next PR.

## Inspector

Open `viz/index.html`. Tabs A / B / C walk the map, the ring stopwatch,
and leftover-aware admit. On C, naive / joint. It plays the goldens. It
does not run the engine.

## Non-goals (v1)

No TUI. No packet-level sim. No trainer, RL, MILP, or OCS.

## Working on this repo

New sessions: `AGENTS.md`, then `STATUS.md`.
