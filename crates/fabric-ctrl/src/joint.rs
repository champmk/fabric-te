//! Joint bindings, evaluate, admit. Transcribed from docs/DESIGN.md §13.

use std::collections::{BTreeMap, BTreeSet};

use fabric_model::JobSpec;
use fabric_sim::{
    k_shortest, phase_duration_ps, water_fill, EventKind, EventPayload, Fel, Path, PathMode,
    Residual,
};
use fabric_topo::Graph;
use fabric_types::{AdmitSeq, BindingKind, GpuAvail, GpuId, JobId, JobState, LinkId, RejectCode};

use crate::table::{
    communicators, rank_map, Binding, BindingNote, Flow, JobRec, JobTable, Occupancy,
};

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

/// Feasible evaluate result. §13.3
#[derive(Clone, Debug)]
pub struct Feasible {
    pub cost: f64,
    pub t_pred: i128,
    pub cir_add: BTreeMap<LinkId, u64>,
    pub paths_chosen: Vec<Path>,
    pub planned: Vec<Flow>,
}

const PRIORITY: [RejectCode; 7] = [
    RejectCode::NoFreeGpus,
    RejectCode::FragmentedGpus,
    RejectCode::DeadElementOnPath,
    RejectCode::ZeroLeftover,
    RejectCode::ResidualExhausted,
    RejectCode::SloMiss,
    RejectCode::CrossRailUnsupported,
];

fn same_node(graph: &Graph, src: GpuId, dst: GpuId) -> bool {
    match (graph.gpu(src), graph.gpu(dst)) {
        (Some(s), Some(d)) => s.node == d.node,
        _ => false,
    }
}

fn rail_of(graph: &Graph, g: GpuId) -> Option<fabric_types::RailId> {
    graph.gpu(g).map(|gpu| gpu.rail)
}

fn leaf_failed(graph: &Graph, id: Option<fabric_types::LeafId>) -> bool {
    match id {
        None => true,
        Some(id) => graph
            .leaves
            .get(id.0 as usize)
            .map(|l| l.failed)
            .unwrap_or(true),
    }
}

fn spine_failed(graph: &Graph, id: fabric_types::SpineId) -> bool {
    graph
        .spines
        .get(id.0 as usize)
        .map(|s| s.failed)
        .unwrap_or(true)
}

fn link_failed(graph: &Graph, e: Option<LinkId>) -> bool {
    match e {
        None => true,
        Some(e) => graph.link(e).map(|l| l.failed).unwrap_or(true),
    }
}

/// src/dst Unavailable, or hs/hd/leaf/spine failed. §13.3
fn dead_element_on_path(graph: &Graph, src: GpuId, dst: GpuId) -> bool {
    match (graph.gpu(src), graph.gpu(dst)) {
        (Some(s), Some(d)) => {
            if s.avail != GpuAvail::Present || d.avail != GpuAvail::Present {
                return true;
            }
        }
        _ => return true,
    }
    if link_failed(graph, graph.host_up(src)) || link_failed(graph, graph.host_down(dst)) {
        return true;
    }
    let sl = graph.leaf_of(src);
    let dl = graph.leaf_of(dst);
    if leaf_failed(graph, sl) || leaf_failed(graph, dl) {
        return true;
    }
    if sl != dl {
        if let (Some(sl), Some(dl)) = (sl, dl) {
            let spines = graph.common_spines(sl, dl);
            if spines.is_empty() || spines.iter().all(|&s| spine_failed(graph, s)) {
                return true;
            }
        }
    }
    false
}

fn rel_eq(a: f64, b: f64, eps: f64) -> bool {
    let scale = a.abs().max(b.abs());
    if scale == 0.0 {
        return true;
    }
    (a - b).abs() <= eps * scale
}

fn phase0_links(planned: &[Flow]) -> Vec<LinkId> {
    let mut out = Vec::new();
    for f in planned.iter().filter(|f| f.comm == 0 && f.phase == 0) {
        out.extend(f.path.links.iter().copied());
    }
    out
}

fn gpu_ids_of(b: &Binding) -> Vec<GpuId> {
    b.map.iter().map(|(_, g)| *g).collect()
}

/// Evaluate one binding on CIR leftover `r_avail`. PathMode::Joint. §13.3
pub fn evaluate(
    b: &Binding,
    job: &JobSpec,
    graph: &Graph,
    residual: &Residual,
) -> Result<Feasible, RejectCode> {
    let comms = communicators(b, job);
    let mut cost = 0.0f64;
    let mut cir_add: BTreeMap<LinkId, u64> = BTreeMap::new();
    let mut t_pred: i128 = 0;
    let mut paths_chosen: Vec<Path> = Vec::new();
    let mut planned: Vec<Flow> = Vec::new();

    for comm in &comms {
        let mut t_comm: i128 = 0;
        let mut phase_loads: Vec<BTreeMap<LinkId, u64>> = Vec::new();
        for phi in 0..comm.n_phases {
            let mut flows: Vec<Flow> = Vec::new();
            for (src, dst) in comm.edges(phi) {
                if same_node(graph, src, dst) {
                    continue;
                }
                let rs = rail_of(graph, src);
                let rd = rail_of(graph, dst);
                if rs != rd && !graph.params.allow_cross_rail {
                    return Err(RejectCode::CrossRailUnsupported);
                }
                let ks = k_shortest(src, dst, graph, residual, 8, PathMode::Joint);
                if ks.is_empty() {
                    if dead_element_on_path(graph, src, dst) {
                        return Err(RejectCode::DeadElementOnPath);
                    }
                    return Err(RejectCode::ZeroLeftover);
                }
                let path = ks[0].clone();
                if path.links.iter().any(|&e| residual.r_avail(e) == 0) {
                    return Err(RejectCode::ZeroLeftover);
                }
                cost += path
                    .links
                    .iter()
                    .map(|&e| residual.cost(graph, e))
                    .sum::<f64>();
                paths_chosen.push(path.clone());
                flows.push(Flow {
                    id: fabric_types::FlowId(0),
                    job: job.id,
                    comm: comm.index,
                    phase: phi,
                    src,
                    dst,
                    path,
                    rate_Bps: 0,
                    bytes: comm.chunk_bytes,
                });
            }
            if flows.is_empty() {
                continue;
            }
            let flow_paths: Vec<Path> = flows.iter().map(|f| f.path.clone()).collect();
            let rates = water_fill(&flow_paths, &residual.r_avail)?;
            let b_eff = rates.iter().copied().min().unwrap_or(0);
            t_comm = t_comm.saturating_add(phase_duration_ps(comm.chunk_bytes, b_eff));
            let mut load: BTreeMap<LinkId, u64> = BTreeMap::new();
            for (f, &rate) in flows.iter_mut().zip(rates.iter()) {
                f.rate_Bps = rate;
                for &e in &f.path.links {
                    *load.entry(e).or_insert(0) =
                        load.get(&e).copied().unwrap_or(0).saturating_add(rate);
                }
            }
            phase_loads.push(load);
            planned.extend(flows);
        }
        if t_comm > t_pred {
            t_pred = t_comm;
        }
        let mut max_phi: BTreeMap<LinkId, u64> = BTreeMap::new();
        for load in &phase_loads {
            for (&e, &rho) in load {
                let slot = max_phi.entry(e).or_insert(0);
                if rho > *slot {
                    *slot = rho;
                }
            }
        }
        for (e, rho) in max_phi {
            let slot = cir_add.entry(e).or_insert(0);
            if rho > *slot {
                *slot = rho;
            }
        }
    }

    if cir_add.iter().any(|(&e, &rho)| rho > residual.r_avail(e)) {
        return Err(RejectCode::ResidualExhausted);
    }
    if t_pred > job.deadline_ps {
        return Err(RejectCode::SloMiss);
    }
    Ok(Feasible {
        cost,
        t_pred,
        cir_add,
        paths_chosen,
        planned,
    })
}

/// §13.5. Never OddRingDegenerate. MixDoesNotFit is load-time only.
pub fn select_code(notes: &[RejectCode], free: usize, p: usize, n_bindings: usize) -> RejectCode {
    if free < p {
        return RejectCode::NoFreeGpus;
    }
    if n_bindings == 0 {
        return RejectCode::FragmentedGpus;
    }
    for code in PRIORITY {
        if notes.contains(&code) {
            return code;
        }
    }
    RejectCode::SloMiss
}

fn better(f_cost: f64, f_idx: usize, best_cost: f64, best_idx: usize) -> bool {
    if f_cost < best_cost * (1.0 - 1e-12) {
        return true;
    }
    rel_eq(f_cost, best_cost, 1e-12) && f_idx < best_idx
}

/// Cheapest feasible of K≤16. Occupy + CIR leftover. §13.4
pub fn joint_admit(
    job: &JobSpec,
    graph: &Graph,
    residual: &mut Residual,
    table: &mut JobTable,
    fel: &mut Fel,
) -> Result<(), RejectCode> {
    let n = graph.params.nodes;
    let r = graph.params.gpus_per_node;
    let p = job.gpu_count as usize;
    let free = graph
        .gpus
        .iter()
        .filter(|g| table.occ.is_free(g.id, graph))
        .count();
    let bindings = generate_bindings(job, &table.occ, graph, n, r);

    let mut best: Option<(usize, Binding, Feasible)> = None;
    let mut notes: Vec<BindingNote> = Vec::new();
    let mut codes: Vec<RejectCode> = Vec::new();

    for (idx, b) in bindings.iter().enumerate() {
        match evaluate(b, job, graph, residual) {
            Ok(f) => {
                notes.push(BindingNote {
                    kind: b.kind,
                    gpu_ids: gpu_ids_of(b),
                    cost: f.cost,
                    t_pred_ps: f.t_pred,
                    code: None,
                    phase0_links: phase0_links(&f.planned),
                });
                let take = match &best {
                    None => true,
                    Some((bi, _, bf)) => better(f.cost, idx, bf.cost, *bi),
                };
                if take {
                    best = Some((idx, b.clone(), f));
                }
            }
            Err(code) => {
                codes.push(code);
                notes.push(BindingNote {
                    kind: b.kind,
                    gpu_ids: gpu_ids_of(b),
                    cost: 0.0,
                    t_pred_ps: 0,
                    code: Some(code),
                    phase0_links: Vec::new(),
                });
            }
        }
    }

    if let Some((idx, binding, f)) = best {
        for &(_, g) in &binding.map {
            table.occ.by_gpu.insert(g, job.id);
        }
        for (&e, &rho) in &f.cir_add {
            if rho > 0 {
                residual.inject_cir(graph, e, rho);
            }
        }
        let admit_seq = AdmitSeq(table.next_admit_seq);
        table.next_admit_seq += 1;
        let arrive_ps = job.arrive.ps;
        let compute_ps = job.compute_ps;
        let id = job.id;
        let t_pred = f.t_pred;
        let paths = f.paths_chosen.clone();
        let cir = f.cir_add.clone();
        let planned = f.planned;
        table.by_id.insert(
            id,
            JobRec {
                spec: job.clone(),
                state: JobState::Computing,
                admit_seq: Some(admit_seq),
                binding: Some(binding),
                paths,
                cir,
                planned,
                t_pred_ps: t_pred,
                step_index: 0,
                steps_done: 0,
                reject: None,
                notes,
                chosen_idx: Some(idx),
            },
        );
        fel.push(
            arrive_ps.saturating_add(compute_ps),
            EventKind::StepBoundary,
            EventPayload::StepBoundary { job: id, step: 0 },
        );
        Ok(())
    } else {
        let code = select_code(&codes, free, p, bindings.len());
        table.by_id.insert(
            job.id,
            JobRec {
                spec: job.clone(),
                state: JobState::Rejected,
                admit_seq: None,
                binding: None,
                paths: Vec::new(),
                cir: BTreeMap::new(),
                planned: Vec::new(),
                t_pred_ps: 0,
                step_index: 0,
                steps_done: 0,
                reject: Some(code),
                notes,
                chosen_idx: None,
            },
        );
        Err(code)
    }
}

fn is_rail0_ls(graph: &Graph, link: &fabric_topo::Link) -> bool {
    use fabric_types::Endpoint::{Leaf, Spine};
    let leaf_id = match (link.src, link.dst) {
        (Leaf(l), Spine(_)) | (Spine(_), Leaf(l)) => l,
        _ => return false,
    };
    graph
        .leaves
        .get(leaf_id.0 as usize)
        .is_some_and(|leaf| leaf.rail.0 == 0)
}

/// Example C (§8.6): N=64, H∪C free, every directed rail-0 LS leftover = 0.
pub fn example_c() -> (Graph, Residual, Occupancy) {
    let graph = Graph::generate(512, 8, 1).expect("n64");
    let mut residual = Residual::new(&graph);
    let mut occ = Occupancy::new();
    let r = graph.params.gpus_per_node;
    let mut keep = BTreeSet::new();
    for n in [0u32, 1, 2, 3, 32, 33, 34, 35] {
        keep.insert(GpuId(n * r));
    }
    for n in 0u32..8 {
        keep.insert(GpuId(n * r + 1));
    }
    for g in &graph.gpus {
        if !keep.contains(&g.id) {
            occ.by_gpu.insert(g.id, JobId(u32::MAX));
        }
    }
    for link in &graph.links {
        if is_rail0_ls(&graph, link) {
            residual.inject_cir(&graph, link.id, Residual::admissible(&graph, link.id));
        }
    }
    (graph, residual, occ)
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

    const M_64_MIB: u64 = 67_108_864;
    /// 3000 µs. §8.6
    const D_J_C: i128 = 3_000_000_000;

    fn c_job(id: u32, seq: u64) -> JobSpec {
        JobSpec {
            id: JobId(id),
            arrive: SimTime { ps: 0, seq },
            gpu_count: 8,
            dp: 8,
            tp: 1,
            pp: 1,
            collective: CollectiveKind::RingAllReduce,
            payload_bytes: M_64_MIB,
            step_count: 1,
            compute_ps: 0,
            deadline_ps: D_J_C,
        }
    }

    #[test]
    fn joint_reject_zero_leftover() {
        let (graph, mut residual, occ) = example_c();
        let mut table = JobTable::new();
        table.occ = occ;
        let mut fel = Fel::new();
        let j1 = c_job(1, 0);
        let j2 = c_job(2, 1);
        joint_admit(&j1, &graph, &mut residual, &mut table, &mut fel).expect("J1");
        let err = joint_admit(&j2, &graph, &mut residual, &mut table, &mut fel).unwrap_err();
        assert_eq!(err, RejectCode::ZeroLeftover);
        let rec = table.by_id.get(&JobId(2)).expect("J2 rec");
        assert_eq!(rec.state, JobState::Rejected);
        assert_eq!(rec.reject, Some(RejectCode::ZeroLeftover));
    }

    #[test]
    fn joint_admit_cheapest_feasible() {
        let (graph, residual, occ) = example_c();
        let j1 = c_job(1, 0);
        let n = graph.params.nodes;
        let r = graph.params.gpus_per_node;
        let bs = generate_bindings(&j1, &occ, &graph, n, r);
        let ff0 = bs
            .iter()
            .find(|b| matches!(b.kind, BindingKind::FirstFitShift { skip_free_gpus: 0 }))
            .expect("FirstFitShift{0}");
        assert_eq!(
            evaluate(ff0, &j1, &graph, &residual).unwrap_err(),
            RejectCode::ZeroLeftover
        );

        let mut residual = residual;
        let mut table = JobTable::new();
        table.occ = occ;
        let mut fel = Fel::new();
        joint_admit(&j1, &graph, &mut residual, &mut table, &mut fel).expect("J1");
        let rec = table.by_id.get(&JobId(1)).expect("rec");
        assert_eq!(
            rec.binding.as_ref().map(|b| b.kind),
            Some(BindingKind::RailRotate { start_rail: 1 })
        );
    }

    #[test]
    fn naive_may_overadmit() {
        let (graph, mut residual, occ) = example_c();
        let mut table = JobTable::new();
        table.occ = occ;
        let mut fel = Fel::new();
        let j1 = c_job(1, 0);
        let j2 = c_job(2, 1);
        crate::naive::naive_admit(&j1, &graph, &mut residual, &mut table, &mut fel).expect("J1");
        crate::naive::naive_admit(&j2, &graph, &mut residual, &mut table, &mut fel).expect("J2");
        // Closed form at leftover 2.5 GB/s is 46.976 ms + 14 µs α = 46990 us.
        let t_25 = fabric_model::ring_allreduce_ps(8, M_64_MIB, 2.5e9);
        assert_eq!(fabric_types::ps_to_us(t_25), 46_990);
        assert_eq!(fabric_types::ps_to_us(t_25) - 14, 46_976);
        // Four concurrent cross-rail hops share that LS; water-fill B_eff = 2.5e9/4.
        let expect = fabric_model::ring_allreduce_ps(8, M_64_MIB, 2.5e9 / 4.0);
        let slop = 1_000_000; // 1 µs
        for id in [JobId(1), JobId(2)] {
            let rec = table.by_id.get(&id).expect("rec");
            assert_eq!(rec.state, JobState::Computing, "job {}", id.0);
            assert!(rec.reject.is_none());
            let t = rec.t_pred_ps;
            assert!(t > D_J_C, "job {} SLO-miss, t={}", id.0, t);
            assert!(
                (t - expect).abs() <= slop || (t - t_25).abs() <= slop,
                "job {} t_pred={} expect_shared={} or 2.5GB/s={} ({} us)",
                id.0,
                t,
                expect,
                t_25,
                fabric_types::ps_to_us(t)
            );
        }
    }

    #[test]
    fn simultaneous_fifo_admit() {
        let (graph, mut residual, occ) = example_c();
        let mut table = JobTable::new();
        table.occ = occ;
        let mut fel = Fel::new();
        fel.push(
            0,
            EventKind::JobArrive,
            fabric_types::EventPayload::JobArrive { job: JobId(1) },
        );
        fel.push(
            0,
            EventKind::JobArrive,
            fabric_types::EventPayload::JobArrive { job: JobId(2) },
        );
        let a = fel.pop().expect("first");
        let b = fel.pop().expect("second");
        assert_eq!(a.t.ps, 0);
        assert_eq!(b.t.ps, 0);
        assert!(a.t.seq < b.t.seq, "lower seq first");
        assert_eq!(
            a.payload,
            fabric_types::EventPayload::JobArrive { job: JobId(1) }
        );
        assert_eq!(
            b.payload,
            fabric_types::EventPayload::JobArrive { job: JobId(2) }
        );

        let j1 = c_job(1, 0);
        let j2 = c_job(2, 1);
        joint_admit(&j1, &graph, &mut residual, &mut table, &mut fel).expect("J1 first");
        assert!(
            residual.cir.iter().any(|&c| c > 0),
            "first admit installs CIR"
        );
        let cir_after = residual.cir.clone();
        let err = joint_admit(&j2, &graph, &mut residual, &mut table, &mut fel).unwrap_err();
        assert_eq!(err, RejectCode::ZeroLeftover);
        assert_eq!(residual.cir, cir_after, "reject does not add CIR");
        assert_eq!(table.by_id.get(&JobId(1)).unwrap().admit_seq.unwrap().0, 0);
        assert!(table.occ.by_gpu.values().any(|&j| j == JobId(1)));
        assert!(!table.occ.by_gpu.values().any(|&j| j == JobId(2)));
    }

    #[test]
    fn scratch_not_used_by_jobs() {
        let (graph, mut residual, occ) = example_c();
        let mut table = JobTable::new();
        table.occ = occ;
        let mut fel = Fel::new();
        joint_admit(&c_job(1, 0), &graph, &mut residual, &mut table, &mut fel).expect("J1");
        for (i, link) in graph.links.iter().enumerate() {
            let cir = residual.cir.get(i).copied().unwrap_or(0);
            let cap95 = link.capacity_Bps * 95 / 100;
            assert!(
                cir <= cap95,
                "I9 link {} cir={} cap95={}",
                link.id.0,
                cir,
                cap95
            );
        }

        let graph = Graph::generate(256, 8, 1).expect("n32");
        let mut residual = Residual::new(&graph);
        let mut table = JobTable::new();
        let mut fel = Fel::new();
        let job = job_p(16);
        joint_admit(&job, &graph, &mut residual, &mut table, &mut fel).expect("empty n32");
        for (i, link) in graph.links.iter().enumerate() {
            let cir = residual.cir.get(i).copied().unwrap_or(0);
            assert!(
                cir <= Residual::admissible(&graph, link.id),
                "I9 empty n32 link {}",
                link.id.0
            );
        }
    }
}
