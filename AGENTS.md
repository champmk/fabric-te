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

## Commands

```
cargo test --workspace
cargo fmt
```

Human-readable spec: `docs/DESIGN.html` (same content as the markdown).
