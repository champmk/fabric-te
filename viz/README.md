# Examples A B C

White figures from `docs/DESIGN.md`. Open `viz/index.html`.

| Tab | What | Steps |
| --- | --- | --- |
| A map | Clos closed form, 256 GPUs | leaves → one node → same-rail hop → counts |
| B ring | AllReduce stopwatch, T = 2362.810 µs | ring → slices → hops → T |
| C leftover | naive vs joint admit | idle → J1 → collective → J2 |

Tabs A / B / C. prev / next or arrows. On C, naive / joint. Keys 1 / 2 / 3 select the example.
