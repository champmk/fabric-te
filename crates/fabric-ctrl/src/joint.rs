//! Joint binding generator. Transcribed from docs/DESIGN.md §13.1–§13.2.

use std::collections::BTreeSet;

use fabric_model::JobSpec;
use fabric_topo::Graph;
use fabric_types::{BindingKind, GpuId};

use crate::table::{rank_map, Binding, Occupancy};

const N_FF: usize = 8;
#[allow(dead_code)]
const N_RAIL: usize = 8;

/// K ≤ 16: 8 first-fit shifts (skip `i*R` free GPUs) + 8 rail rotates.
/// De-dup by sorted GPU-set. Does not evaluate or admit.
pub fn generate_bindings(
    job: &JobSpec,
    occ: &Occupancy,
    graph: &Graph,
    n: u32,
    r: u32,
) -> Vec<Binding> {
    let p = job.gpu_count as usize;
    let mut out: Vec<Binding> = Vec::new();
    let mut seen: BTreeSet<Vec<GpuId>> = BTreeSet::new();
    let scan: Vec<GpuId> = (0..n)
        .flat_map(|ni| (0..r).map(move |ri| GpuId(ni * r + ri)))
        .collect();
    let free: Vec<GpuId> = scan
        .into_iter()
        .filter(|&g| occ.is_free(g, graph))
        .collect();

    for i in 0..N_FF {
        let skip = i * r as usize;
        if skip.saturating_add(p) > free.len() {
            break;
        }
        let pick = &free[skip..skip + p];
        if pick.len() == p {
            let mut sorted = pick.to_vec();
            sorted.sort();
            if seen.insert(sorted) {
                out.push(Binding {
                    kind: BindingKind::FirstFitShift {
                        skip_free_gpus: skip as u8,
                    },
                    map: rank_map(pick, job, graph),
                });
            }
        }
    }

    // R == 8 == N_RAIL on v1 Clos.
    for rot in 0..r {
        let mut pick: Vec<GpuId> = Vec::new();
        'rot: for off in 0..r {
            let rail = (rot + off) % r;
            for ni in 0..n {
                let g = GpuId(ni * r + rail);
                if occ.is_free(g, graph) {
                    pick.push(g);
                }
                if pick.len() == p {
                    break 'rot;
                }
            }
            if pick.len() == p {
                break;
            }
        }
        if pick.len() == p {
            let mut sorted = pick.clone();
            sorted.sort();
            if seen.insert(sorted) {
                out.push(Binding {
                    kind: BindingKind::RailRotate {
                        start_rail: rot as u8,
                    },
                    map: rank_map(&pick, job, graph),
                });
            }
        }
    }

    out.truncate(16);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_types::{CollectiveKind, JobId, SimTime};

    fn job_p(gpu_count: u32) -> JobSpec {
        JobSpec {
            id: JobId(1),
            arrive: SimTime { ps: 0, seq: 0 },
            gpu_count,
            dp: gpu_count,
            tp: 1,
            pp: 1,
            collective: CollectiveKind::RingAllReduce,
            payload_bytes: 64,
            step_count: 1,
            compute_ps: 0,
            deadline_ps: i128::MAX,
        }
    }

    #[test]
    fn joint_k16_bound() {
        let graph = Graph::generate(256, 8, 1).expect("n32");
        let occ = Occupancy::new();
        let n = graph.params.nodes;
        let r = graph.params.gpus_per_node;
        assert_eq!((n, r), (32, 8));
        let job = job_p(16);
        let bs = generate_bindings(&job, &occ, &graph, n, r);
        assert!(bs.len() <= 16, "K cap, got {}", bs.len());
        // Empty n32, p=16: 8 first-fit shifts + 8 rail rotates, no de-dup.
        assert_eq!(bs.len(), 16);

        let ff: Vec<u8> = bs
            .iter()
            .filter_map(|b| match b.kind {
                BindingKind::FirstFitShift { skip_free_gpus } => Some(skip_free_gpus),
                _ => None,
            })
            .collect();
        // skip_free_gpus is i*R (0,8,16,…) — not i and not i nodes.
        assert_eq!(ff, vec![0, 8, 16, 24, 32, 40, 48, 56]);
        assert_ne!(ff, vec![0, 1, 2, 3, 4, 5, 6, 7]);

        let rails: Vec<u8> = bs
            .iter()
            .filter_map(|b| match b.kind {
                BindingKind::RailRotate { start_rail } => Some(start_rail),
                _ => None,
            })
            .collect();
        assert_eq!(rails, vec![0, 1, 2, 3, 4, 5, 6, 7]);
    }
}
