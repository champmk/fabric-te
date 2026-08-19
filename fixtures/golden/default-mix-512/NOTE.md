# default-mix-512: joint vs naive

Joint does not beat naive on `hotspot_us`. Completions match.

| | naive | joint |
| --- | --- | --- |
| admits / rejects | 21 / 0 | 21 / 0 |
| completions_by_deadline | 21 | 21 |
| hotspot_us | 47_315_040 | 49_801_600 |
| last_flow_collective_us_max | 1344 | 1414 |

Joint CIR leftover is `c_e * 95/100` (I9, scratch closed). Naive water-fills `c_e − cir` (scratch open). Same first-fit 16-GPU maps land on two full nodes; ring `dp=2` is host-only. Isolated T at 47.5 GB/s is 1414.818 µs vs 1344.177 µs at 50 GB/s. Host links sit at ≥80% `c_e` for the longer collective, so hotspot scales by ~1414/1344 ≈ 1.052. Completions stay 21: both T values are under `D_j = 5 ms`.
