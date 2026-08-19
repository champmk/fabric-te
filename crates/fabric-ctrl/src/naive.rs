//! Naive first-fit admit. Transcribed from docs/DESIGN.md §12, §10.3–§10.4.

use std::collections::BTreeMap;

use fabric_model::JobSpec;
use fabric_sim::{
    k_shortest, s_to_ps, water_fill, Event, EventKind, EventPayload, Fel, Path, PathMode, Residual,
};
use fabric_topo::Graph;
use fabric_types::{AdmitSeq, BindingKind, GpuId, JobState, LinkId, RejectCode};

use crate::table::{communicators, rank_map, Binding, Flow, JobRec, JobTable, Occupancy};

/// Node-major, then local rank. GpuId = n*R+r.
pub fn gpu_scan_order(n: u32, r: u32) -> impl Iterator<Item = GpuId> {
    (0..n).flat_map(move |ni| (0..r).map(move |ri| GpuId(ni * r + ri)))
}

pub fn first_fit(occ: &Occupancy, graph: &Graph, gpu_count: u32) -> Vec<GpuId> {
    gpu_scan_order(graph.params.nodes, graph.params.gpus_per_node)
        .filter(|&g| occ.is_free(g, graph))
        .take(gpu_count as usize)
        .collect()
}

/// Leftover is `c_e − cir` (scratch open). Failed link → 0. §12.4
fn physical_leftover(graph: &Graph, residual: &Residual) -> Vec<u64> {
    graph
        .links
        .iter()
        .map(|link| {
            let cir = residual.cir.get(link.id.0 as usize).copied().unwrap_or(0);
            if link.failed {
                0
            } else {
                link.capacity_Bps.saturating_sub(cir)
            }
        })
        .collect()
}

fn t_phase_ps(chunk_bytes: u64, b_eff: u64) -> i128 {
    if b_eff == 0 {
        i128::MAX
    } else {
        s_to_ps(1e-6 + (chunk_bytes as f64) / (b_eff as f64))
    }
}

fn same_node(graph: &Graph, src: GpuId, dst: GpuId) -> bool {
    match (graph.gpu(src), graph.gpu(dst)) {
        (Some(s), Some(d)) => s.node == d.node,
        _ => false,
    }
}

/// First-fit + ECMP. Only `NoFreeGpus` rejects. Water-fill `Err` still admits. §12
pub fn naive_admit(
    job: &JobSpec,
    graph: &Graph,
    residual: &mut Residual,
    table: &mut JobTable,
    fel: &mut Fel,
) -> Result<(), RejectCode> {
    let picked = first_fit(&table.occ, graph, job.gpu_count);
    if picked.len() < job.gpu_count as usize {
        table.by_id.insert(
            job.id,
            JobRec {
                spec: job.clone(),
                state: JobState::Rejected,
                admit_seq: None,
                binding: None,
                paths: Vec::new(),
                cir: BTreeMap::new(),
                t_pred_ps: 0,
                step_index: 0,
                steps_done: 0,
                reject: Some(RejectCode::NoFreeGpus),
            },
        );
        return Err(RejectCode::NoFreeGpus);
    }

    let binding = Binding {
        kind: BindingKind::NaiveFirstFit,
        map: rank_map(&picked, job, graph),
    };
    let comms = communicators(&binding, job);
    let leftover = physical_leftover(graph, residual);

    let mut paths = Vec::new();
    let mut cir: BTreeMap<LinkId, u64> = BTreeMap::new();
    let mut t_pred: i128 = 0;
    let mut wf_fail = false;
    let mut next_flow = 0u64;

    for comm in &comms {
        let mut t_comm: i128 = 0;
        let mut comm_has_fabric = false;
        for phi in 0..comm.n_phases {
            let mut flows: Vec<Flow> = Vec::new();
            for (src, dst) in comm.edges(phi) {
                if same_node(graph, src, dst) {
                    continue;
                }
                comm_has_fabric = true;
                let path = k_shortest(src, dst, graph, residual, 8, PathMode::Ecmp)
                    .into_iter()
                    .next()
                    .unwrap_or_else(Path::empty);
                paths.push(path.clone());
                flows.push(Flow {
                    id: fabric_types::FlowId(next_flow),
                    job: job.id,
                    phase: phi,
                    src,
                    dst,
                    path,
                    rate_Bps: 0,
                    bytes: comm.chunk_bytes,
                });
                next_flow += 1;
            }
            if flows.is_empty() {
                continue;
            }
            let flow_paths: Vec<Path> = flows.iter().map(|f| f.path.clone()).collect();
            match water_fill(&flow_paths, &leftover) {
                Ok(rates) => {
                    let b_eff = rates.iter().copied().min().unwrap_or(0);
                    if b_eff == 0 {
                        wf_fail = true;
                    } else {
                        t_comm = t_comm.saturating_add(t_phase_ps(comm.chunk_bytes, b_eff));
                    }
                    let mut load: BTreeMap<LinkId, u64> = BTreeMap::new();
                    for (f, &rate) in flows.iter().zip(rates.iter()) {
                        for &e in &f.path.links {
                            let slot = load.entry(e).or_insert(0);
                            *slot = slot.saturating_add(rate);
                        }
                    }
                    for (e, rho) in load {
                        let slot = cir.entry(e).or_insert(0);
                        if rho > *slot {
                            *slot = rho;
                        }
                    }
                }
                Err(_) => {
                    wf_fail = true;
                }
            }
        }
        if comm_has_fabric && t_comm > t_pred {
            t_pred = t_comm;
        }
    }

    if wf_fail {
        t_pred = i128::MAX;
        cir.clear();
    }

    for &(_, g) in &binding.map {
        table.occ.by_gpu.insert(g, job.id);
    }
    for (&e, &rho) in &cir {
        if rho > 0 {
            residual.inject_cir(graph, e, rho);
        }
    }
    let admit_seq = AdmitSeq(table.next_admit_seq);
    table.next_admit_seq += 1;
    let arrive_ps = job.arrive.ps;
    let compute_ps = job.compute_ps;
    let id = job.id;
    table.by_id.insert(
        id,
        JobRec {
            spec: job.clone(),
            state: JobState::Computing,
            admit_seq: Some(admit_seq),
            binding: Some(binding),
            paths,
            cir,
            t_pred_ps: t_pred,
            step_index: 0,
            steps_done: 0,
            reject: None,
        },
    );
    fel.push(
        arrive_ps.saturating_add(compute_ps),
        EventKind::StepBoundary,
        EventPayload::StepBoundary { job: id, step: 0 },
    );
    Ok(())
}

/// StepBoundary: Computing → Collecting; CollectiveStart at the same ps.
pub fn pump_until_collective_start(table: &mut JobTable, fel: &mut Fel) -> Option<Event> {
    while let Some(e) = fel.pop() {
        match e.payload {
            EventPayload::StepBoundary { job, step } => {
                if let Some(rec) = table.by_id.get_mut(&job) {
                    rec.state = JobState::Collecting;
                }
                fel.push(
                    e.t.ps,
                    EventKind::CollectiveStart,
                    EventPayload::CollectiveStart { job, step },
                );
            }
            EventPayload::CollectiveStart { .. } => return Some(e),
            _ => return Some(e),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_topo::Graph;
    use fabric_types::{CollectiveKind, JobId, SimTime};

    fn tiny_job(deadline_ps: i128, compute_ps: i128) -> JobSpec {
        JobSpec {
            id: JobId(1),
            arrive: SimTime { ps: 0, seq: 0 },
            gpu_count: 2,
            dp: 2,
            tp: 1,
            pp: 1,
            collective: CollectiveKind::RingAllReduce,
            payload_bytes: 64,
            step_count: 1,
            compute_ps,
            deadline_ps,
        }
    }

    #[test]
    fn naive_scan_order_node_then_rank() {
        let graph = Graph::generate(256, 8, 1).expect("n32");
        let occ = Occupancy::new();
        let first10: Vec<GpuId> = gpu_scan_order(graph.params.nodes, graph.params.gpus_per_node)
            .filter(|&g| occ.is_free(g, &graph))
            .take(10)
            .collect();
        assert_eq!(first10, (0..10).map(GpuId).collect::<Vec<_>>());
    }

    #[test]
    fn naive_admit_gpu_count_only() {
        let graph = Graph::generate(256, 8, 1).expect("n32");
        let mut residual = Residual::new(&graph);
        let mut table = JobTable::new();
        let mut fel = Fel::new();
        // Isolated T at 47.5 GB/s for ring p=2 is ≫ 1 ps.
        let job = tiny_job(1, 0);
        assert!(naive_admit(&job, &graph, &mut residual, &mut table, &mut fel).is_ok());
        let rec = table.by_id.get(&JobId(1)).expect("rec");
        assert_eq!(rec.state, JobState::Computing);
        assert!(rec.reject.is_none());
        assert_eq!(table.occ.by_gpu.len(), 2);
        assert_eq!(table.occ.by_gpu.get(&GpuId(0)), Some(&JobId(1)));
        assert_eq!(table.occ.by_gpu.get(&GpuId(1)), Some(&JobId(1)));
    }

    #[test]
    fn compute_before_first_collective() {
        let graph = Graph::generate(256, 8, 1).expect("n32");
        let mut residual = Residual::new(&graph);
        let mut table = JobTable::new();
        let mut fel = Fel::new();
        let compute_ps = 10_000_000_000;
        let job = tiny_job(1, compute_ps);
        naive_admit(&job, &graph, &mut residual, &mut table, &mut fel).expect("admit");
        let cs = pump_until_collective_start(&mut table, &mut fel).expect("CollectiveStart");
        assert_eq!(cs.kind, EventKind::CollectiveStart);
        assert_eq!(cs.t.ps, job.arrive.ps + compute_ps);
        let rec = table.by_id.get(&JobId(1)).expect("rec");
        assert_eq!(rec.state, JobState::Collecting);
    }
}
