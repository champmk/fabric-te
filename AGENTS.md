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
