# fabric-te — session bootstrap

Read these **before writing or planning code**. Do not reconstruct the design from chat.

1. `STATUS.md` — what is done, what is next, what changed.
2. `docs/DESIGN.md` — source of truth. Implementation is transcription.
3. For the current PR only: the sections `STATUS.md` lists under **Read for next**.

## Rules

- Spec first. If code wants a new behavior, edit `docs/DESIGN.md` and `STATUS.md`, then code.
- Do not invent types, formulas, CLI flags, or reject codes.
- One PR at a time. Do not start the next PR until that PR’s tests pass.
- Naive may miss the network SLO. Joint may not.
- No satellite / LEO / space-network content.
- No packet-level sim, trainer, RL, OCS, or visual globe in v1.
- Update `STATUS.md` in the same change that lands or abandons work.

## Session loop (you code, user reviews)

1. Read `STATUS.md` **Next**. Only that PR (or a parallel *crate* pair).
2. **Parallel only when the DAG allows** (independent crates, same parent). Write in worktrees. Land **in series**: merge A, rebase B onto master, then open/update B. Never two PRs that both rewrite `Cargo.toml` / `STATUS.md` onto `master`.
3. **Sequential** (the rest of this repo after PR3): one PR.
4. Implement the whole slice. Do not start review mid-write.
5. `/code-review` on that PR’s diff. Fix **block** and **should**.
6. `/security-review` on the updated diff. Fix real findings.
7. Push a **draft** PR. Body = Problem / Solution / Review Order / Testing (below).
8. Reply: one sentence what landed, one sentence implication, PR link.
9. **Stop.** Do not start the next PR until the user reviews / merges (or says continue).

Agent scratch notes go in `.scratch/` (gitignored). Never commit `PR*_DONE.md`.

## PR description (always)

Every GitHub PR body uses exactly these four headings. Caveman: short fragments, no filler, no restating the spec.

```
## Problem
<why this slice exists. 1–2 lines.>

## Solution
<what landed. 1–3 lines. files/crates, not essay.>

## Review Order
1. <file — why first>
2. ...

## Testing
<exact commands + named tests. expected exit if it matters.>
```

Do not add extra sections. Do not write this format only when asked.

## Commands

```
cargo test --workspace
cargo fmt
```

Human-readable spec: `docs/DESIGN.html` (same content as the markdown).
