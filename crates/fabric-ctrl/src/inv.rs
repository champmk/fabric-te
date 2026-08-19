//! I1–I10. Transcribed from docs/DESIGN.md §18.2.

use std::collections::BTreeSet;

use fabric_model::{pairwise_alltoall_ps, ring_allreduce_ps};
use fabric_sim::{s_to_ps, Path, Residual};
use fabric_topo::Graph;
use fabric_types::{CollectiveKind, Endpoint, JobState, LinkId, Policy};

use crate::epoch::{i2_holds, i3_holds};
use crate::table::JobTable;

pub const IDS: [&str; 10] = ["I1", "I2", "I3", "I4", "I5", "I6", "I7", "I8", "I9", "I10"];

pub struct Ctx<'a> {
    pub graph: &'a Graph,
    pub residual: &'a Residual,
    pub table: &'a JobTable,
    pub policy: Policy,
    pub inflight_load: &'a [u64],
    pub bytes_epoch: &'a [u64],
    pub inflight_paths: &'a [Path],
}

pub use fabric_trace::TraceRollup;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveCounters {
    pub arrivals: u64,
    pub admits: u64,
    pub rejects: u64,
    pub kills: u64,
    pub completes: u64,
}

pub fn i1_holds(ctx: &Ctx<'_>) -> bool {
    for (i, link) in ctx.graph.links.iter().enumerate() {
        let load = ctx.inflight_load.get(i).copied().unwrap_or(0);
        if load > link.capacity_Bps {
            return false;
        }
    }
    match ctx.policy {
        Policy::Naive => true,
        Policy::Joint => ctx.graph.links.iter().all(|link| {
            let cir = ctx
                .residual
                .cir
                .get(link.id.0 as usize)
                .copied()
                .unwrap_or(0);
            cir <= Residual::admissible(ctx.graph, link.id)
        }),
    }
}

pub fn i2_ok(ctx: &Ctx<'_>) -> bool {
    i2_holds(ctx.graph, ctx.bytes_epoch)
}

pub fn i3_ok(ctx: &Ctx<'_>) -> bool {
    i3_holds(ctx.graph, ctx.table)
        && ctx
            .inflight_paths
            .iter()
            .all(|p| !path_hits_failed(ctx.graph, &p.links))
}

pub fn i4_holds(graph: &Graph, table: &JobTable) -> bool {
    for g in table.occ.by_gpu.keys() {
        if graph.gpu(*g).is_none() {
            return false;
        }
    }
    for rec in table.by_id.values() {
        let Some(b) = rec.binding.as_ref() else {
            continue;
        };
        if b.map.len() != rec.spec.gpu_count as usize {
            return false;
        }
        for (_, g) in &b.map {
            if graph.gpu(*g).is_none() {
                return false;
            }
        }
    }
    true
}

pub fn i5_holds(table: &JobTable) -> bool {
    let mut seen = BTreeSet::new();
    for (id, rec) in &table.by_id {
        if !live(rec) {
            continue;
        }
        let Some(b) = rec.binding.as_ref() else {
            continue;
        };
        for (_, g) in &b.map {
            if !seen.insert(*g) {
                return false;
            }
            if table.occ.by_gpu.get(g) != Some(id) {
                return false;
            }
        }
    }
    true
}

pub fn i6_holds(live: &LiveCounters, log: &TraceRollup) -> bool {
    live.arrivals == log.arrivals
        && live.admits == log.admits
        && live.rejects == log.rejects
        && live.kills == log.kills
        && live.completes == log.completes
}

/// I7: equal-B_eff phase sum vs closed form, ±1 ps. Same cases as `model_phase_sum_eq_closed`.
pub fn i7_holds() -> bool {
    i7_case(
        8,
        67_108_864,
        50_000_000_000.0,
        CollectiveKind::RingAllReduce,
    ) && i7_case(
        8,
        67_108_864,
        50_000_000_000.0,
        CollectiveKind::PairwiseAllToAll,
    )
}

fn i7_case(p: u32, payload: u64, b_eff: f64, kind: CollectiveKind) -> bool {
    let closed = match kind {
        CollectiveKind::RingAllReduce => ring_allreduce_ps(p, payload, b_eff),
        CollectiveKind::PairwiseAllToAll => pairwise_alltoall_ps(p, payload, b_eff),
    };
    let n_phases = match kind {
        CollectiveKind::RingAllReduce => 2 * p.saturating_sub(1),
        CollectiveKind::PairwiseAllToAll => p.saturating_sub(1),
    };
    let chunk = (payload as f64) / f64::from(p.max(1));
    let d = 1e-6 + chunk / b_eff;
    let sum = s_to_ps(f64::from(n_phases) * d);
    (sum - closed).abs() <= 1
}

pub fn i8_holds(prev: Option<(i128, u64)>, cur: (i128, u64)) -> bool {
    match prev {
        None => true,
        Some((pps, pseq)) => cur.0 > pps || (cur.0 == pps && cur.1 > pseq),
    }
}

pub fn i9_holds(graph: &Graph, residual: &Residual, policy: Policy) -> bool {
    if policy != Policy::Joint {
        return true;
    }
    graph.links.iter().all(|link| {
        let cir = residual.cir.get(link.id.0 as usize).copied().unwrap_or(0);
        cir <= Residual::admissible(graph, link.id)
    })
}

/// I10: node's R NICs map to R distinct rails and R distinct leaves.
pub fn i10_holds(graph: &Graph) -> bool {
    let r = graph.params.rails as usize;
    for node in &graph.nodes {
        if node.gpus.len() != r {
            return false;
        }
        let mut rails = BTreeSet::new();
        let mut leaves = BTreeSet::new();
        for &gid in &node.gpus {
            let Some(gpu) = graph.gpu(gid) else {
                return false;
            };
            rails.insert(gpu.rail.0);
            let Some(up) = graph.host_up(gid) else {
                return false;
            };
            let Some(link) = graph.link(up) else {
                return false;
            };
            match link.dst {
                Endpoint::Leaf(l) => {
                    leaves.insert(l.0);
                }
                _ => return false,
            }
        }
        if rails.len() != r || leaves.len() != r {
            return false;
        }
    }
    true
}

pub fn path_hits_failed(graph: &Graph, links: &[LinkId]) -> bool {
    links.iter().any(|&e| {
        let Some(l) = graph.link(e) else {
            return true;
        };
        if l.failed {
            return true;
        }
        endpoint_failed(graph, l.src) || endpoint_failed(graph, l.dst)
    })
}

fn endpoint_failed(graph: &Graph, ep: Endpoint) -> bool {
    match ep {
        Endpoint::Spine(s) => graph
            .spines
            .get(s.0 as usize)
            .map(|x| x.failed)
            .unwrap_or(true),
        Endpoint::Leaf(l) => graph
            .leaves
            .get(l.0 as usize)
            .map(|x| x.failed)
            .unwrap_or(true),
        Endpoint::Nic(n) => graph
            .gpu(fabric_types::GpuId(n.0))
            .map(|g| g.avail != fabric_types::GpuAvail::Present)
            .unwrap_or(true),
    }
}

fn live(rec: &crate::table::JobRec) -> bool {
    matches!(rec.state, JobState::Computing | JobState::Collecting)
}

pub fn named_broken(ctx: &Ctx<'_>, id: &str) -> bool {
    match id {
        "I1" => !i1_holds(ctx),
        "I2" => !i2_ok(ctx),
        "I3" => !i3_ok(ctx),
        "I4" => !i4_holds(ctx.graph, ctx.table),
        "I5" => !i5_holds(ctx.table),
        "I7" => !i7_holds(),
        "I9" => !i9_holds(ctx.graph, ctx.residual, ctx.policy),
        "I10" => !i10_holds(ctx.graph),
        _ => false,
    }
}

/// First broken mutate-time invariant, I1–I10 order (skip I6/I8).
pub fn first_mutate_broken(ctx: &Ctx<'_>) -> Option<&'static str> {
    IDS.iter()
        .copied()
        .find(|id| *id != "I6" && *id != "I8" && named_broken(ctx, id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{Binding, JobRec};
    use fabric_model::JobSpec;
    use fabric_types::{BindingKind, GpuId, JobId, Rank, SimTime};

    fn n32() -> Graph {
        Graph::generate(256, 8, 1).expect("n32")
    }

    fn empty_ctx<'a>(
        graph: &'a Graph,
        residual: &'a Residual,
        table: &'a JobTable,
        load: &'a [u64],
        bytes: &'a [u64],
        paths: &'a [Path],
        policy: Policy,
    ) -> Ctx<'a> {
        Ctx {
            graph,
            residual,
            table,
            policy,
            inflight_load: load,
            bytes_epoch: bytes,
            inflight_paths: paths,
        }
    }

    #[test]
    fn i7_eq_closed_form() {
        assert!(i7_holds());
    }

    #[test]
    fn i10_generated_clos() {
        assert!(i10_holds(&n32()));
        assert!(i10_holds(&Graph::generate(512, 8, 1).expect("n64")));
    }

    #[test]
    fn i1_naive_load_over_cap() {
        let graph = n32();
        let residual = Residual::new(&graph);
        let table = JobTable::new();
        let mut load = vec![0u64; graph.links.len()];
        load[0] = graph.links[0].capacity_Bps + 1;
        let bytes = vec![0u64; graph.links.len()];
        let paths: [Path; 0] = [];
        let ctx = empty_ctx(
            &graph,
            &residual,
            &table,
            &load,
            &bytes,
            &paths,
            Policy::Naive,
        );
        assert!(!i1_holds(&ctx));
        assert_eq!(first_mutate_broken(&ctx), Some("I1"));
    }

    #[test]
    fn i4_gpu_outside_cluster() {
        let graph = n32();
        let mut table = JobTable::new();
        table.occ.by_gpu.insert(GpuId(999_999), JobId(1));
        assert!(!i4_holds(&graph, &table));
    }

    #[test]
    fn i5_two_live_jobs_one_gpu() {
        let graph = n32();
        let mut table = JobTable::new();
        let spec = |id: u32| JobSpec {
            id: JobId(id),
            arrive: SimTime { ps: 0, seq: 0 },
            gpu_count: 1,
            dp: 1,
            tp: 1,
            pp: 1,
            collective: CollectiveKind::RingAllReduce,
            payload_bytes: 64,
            step_count: 1,
            compute_ps: 0,
            deadline_ps: 1,
        };
        let bind = Binding {
            kind: BindingKind::NaiveFirstFit,
            map: vec![(Rank(0), GpuId(0))],
        };
        for id in [1u32, 2] {
            table.by_id.insert(
                JobId(id),
                JobRec {
                    spec: spec(id),
                    state: JobState::Computing,
                    admit_seq: None,
                    binding: Some(bind.clone()),
                    paths: Vec::new(),
                    cir: Default::default(),
                    planned: Vec::new(),
                    t_pred_ps: 0,
                    step_index: 0,
                    steps_done: 0,
                    reject: None,
                    notes: Vec::new(),
                    chosen_idx: None,
                },
            );
        }
        table.occ.by_gpu.insert(GpuId(0), JobId(1));
        assert!(!i5_holds(&table));
        assert!(i4_holds(&graph, &table));
    }

    #[test]
    fn i8_ps_monotonic_unique() {
        assert!(i8_holds(None, (0, 0)));
        assert!(i8_holds(Some((0, 0)), (0, 1)));
        assert!(i8_holds(Some((0, 1)), (10, 0)));
        assert!(!i8_holds(Some((10, 0)), (10, 0)));
        assert!(!i8_holds(Some((10, 3)), (9, 99)));
    }

    #[test]
    fn i9_joint_scratch() {
        let graph = n32();
        let mut residual = Residual::new(&graph);
        let e = graph.links[0].id;
        residual.inject_cir(&graph, e, Residual::admissible(&graph, e) + 1);
        assert!(!i9_holds(&graph, &residual, Policy::Joint));
        assert!(i9_holds(&graph, &residual, Policy::Naive));
    }

    #[test]
    fn i6_rollup_eq() {
        let live = LiveCounters {
            arrivals: 2,
            admits: 1,
            rejects: 1,
            kills: 0,
            completes: 1,
        };
        let log = TraceRollup {
            arrivals: 2,
            admits: 1,
            rejects: 1,
            kills: 0,
            completes: 1,
        };
        assert!(i6_holds(&live, &log));
        let mut bad = log.clone();
        bad.arrivals = 3;
        assert!(!i6_holds(&live, &bad));
    }

    #[test]
    fn empty_n32_all_mutate_ok() {
        let graph = n32();
        let residual = Residual::new(&graph);
        let table = JobTable::new();
        let load = vec![0u64; graph.links.len()];
        let bytes = vec![0u64; graph.links.len()];
        let paths: [Path; 0] = [];
        let ctx = empty_ctx(
            &graph,
            &residual,
            &table,
            &load,
            &bytes,
            &paths,
            Policy::Joint,
        );
        assert_eq!(first_mutate_broken(&ctx), None);
        assert!(i4_holds(&graph, &table));
    }
}
