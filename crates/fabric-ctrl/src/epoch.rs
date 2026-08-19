//! 2PC failure epoch. Transcribed from docs/DESIGN.md §14.
//! Prepare is pure: clone Graph, no live mutation.

use std::collections::BTreeMap;

use fabric_sim::{
    k_shortest, phase_duration_ps, water_fill, Event, EventKind, EventPayload, Path, PathMode,
    Residual,
};
use fabric_topo::{Graph, Link};
use fabric_types::{
    Endpoint, EpochId, GpuAvail, GpuId, JobId, JobState, LeafId, LinkId, NicId, Policy, RailId,
    RejectCode, SpineId, UnavailReason,
};

use crate::joint::evaluate;
use crate::table::{communicators, Flow, JobRec, JobTable};

const DEFAULT_FAIL_PS: i128 = 1;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FailKind {
    Spine(SpineId),
    Leaf(LeafId),
    Rail(RailId),
    Link(LinkId),
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct FailSpec {
    pub t_ps: i128,
    pub kind: FailKind,
}

impl FailSpec {
    pub fn event(&self) -> (EventKind, EventPayload) {
        match self.kind {
            FailKind::Spine(spine) => (EventKind::SpineFail, EventPayload::SpineFail { spine }),
            FailKind::Leaf(leaf) => (EventKind::LeafFail, EventPayload::LeafFail { leaf }),
            FailKind::Rail(rail) => (EventKind::RailFail, EventPayload::RailFail { rail }),
            FailKind::Link(link) => (EventKind::LinkFail, EventPayload::LinkFail { link }),
        }
    }
}

/// `--fail` grammar §14.1. Missing `@t` ⇒ 1 ps.
pub fn parse_fail_spec(s: &str) -> Result<FailSpec, String> {
    let (head, time) = match s.split_once('@') {
        Some((h, t)) => (h, Some(t)),
        None => (s, None),
    };
    let (kind, id) = head
        .split_once('=')
        .ok_or_else(|| format!("expected kind=id, got {s}"))?;
    let id_u: u32 = id.parse().map_err(|_| format!("bad fail id {id}"))?;
    let kind = match kind {
        "spine" => FailKind::Spine(SpineId(id_u)),
        "leaf" => FailKind::Leaf(LeafId(id_u)),
        "rail" => {
            let r = u8::try_from(id_u).map_err(|_| format!("bad fail id {id}"))?;
            FailKind::Rail(RailId(r))
        }
        "link" => FailKind::Link(LinkId(id_u)),
        _ => return Err(format!("unknown fail kind {kind}")),
    };
    let t_ps = match time {
        None => DEFAULT_FAIL_PS,
        Some(t) => parse_fail_time(t)?,
    };
    Ok(FailSpec { t_ps, kind })
}

fn parse_fail_time(s: &str) -> Result<i128, String> {
    let unit_at = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
    let (num, unit) = s.split_at(unit_at);
    let x: f64 = num.parse().map_err(|_| format!("bad fail time {s}"))?;
    if !x.is_finite() {
        return Err(format!("non-finite fail time {s}"));
    }
    let secs = match unit {
        "" | "s" => x,
        "ms" => x * 1e-3,
        "us" => x * 1e-6,
        "ns" => x * 1e-9,
        "ps" => x * 1e-12,
        _ => return Err(format!("bad fail time unit {unit}")),
    };
    Ok((secs * 1e12).round_ties_even() as i128)
}

#[derive(Clone, Debug)]
pub struct Reroute {
    pub job: JobId,
    pub paths: Vec<Path>,
    pub planned: Vec<Flow>,
    pub cir: BTreeMap<LinkId, u64>,
    pub t_pred_ps: i128,
}

#[derive(Clone, Debug)]
pub struct EpochPlan {
    pub graph: Graph,
    pub residual: Residual,
    pub kills: Vec<JobId>,
    pub reroutes: Vec<Reroute>,
    pub from: EpochId,
    pub to: EpochId,
}

enum Class {
    Kill,
    Spine,
    Keep,
}

fn live(rec: &JobRec) -> bool {
    matches!(rec.state, JobState::Computing | JobState::Collecting)
}

fn admit_key(rec: &JobRec) -> u64 {
    rec.admit_seq.map(|s| s.0).unwrap_or(u64::MAX)
}

fn live_ids(table: &JobTable) -> Vec<JobId> {
    let mut ids: Vec<JobId> = table
        .by_id
        .iter()
        .filter(|(_, r)| live(r))
        .map(|(id, _)| *id)
        .collect();
    ids.sort_by_key(|id| table.by_id.get(id).map(admit_key).unwrap_or(u64::MAX));
    ids
}

fn nic_of_host(link: &Link) -> Option<NicId> {
    match (link.src, link.dst) {
        (Endpoint::Nic(n), _) | (_, Endpoint::Nic(n)) => Some(n),
        _ => None,
    }
}

fn is_host_link(link: &Link) -> bool {
    nic_of_host(link).is_some()
}

fn is_ls_link(link: &Link) -> bool {
    matches!(
        (link.src, link.dst),
        (Endpoint::Leaf(_), Endpoint::Spine(_)) | (Endpoint::Spine(_), Endpoint::Leaf(_))
    )
}

fn mark_link_dead(link: &mut Link) {
    link.failed = true;
    link.capacity_Bps = 0;
    link.bytes_this_epoch = 0;
}

fn fail_gpu_nic(graph: &mut Graph, g: GpuId) {
    if let Some(gpu) = graph.gpus.get_mut(g.0 as usize) {
        if gpu.id == g {
            gpu.avail = GpuAvail::Unavailable(UnavailReason::FailedNic);
        }
    }
}

fn fail_leaf(graph: &mut Graph, leaf: LeafId) {
    if let Some(l) = graph.leaves.get_mut(leaf.0 as usize) {
        l.failed = true;
    }
    for link in &mut graph.links {
        let hit = match (link.src, link.dst) {
            (Endpoint::Leaf(id), _) | (_, Endpoint::Leaf(id)) => id == leaf,
            _ => false,
        };
        if hit {
            mark_link_dead(link);
        }
    }
    let gpus: Vec<GpuId> = graph
        .gpus
        .iter()
        .filter(|g| graph.leaf_of(g.id) == Some(leaf))
        .map(|g| g.id)
        .collect();
    for g in gpus {
        fail_gpu_nic(graph, g);
    }
}

fn fail_spine(graph: &mut Graph, spine: SpineId) {
    if let Some(s) = graph.spines.get_mut(spine.0 as usize) {
        s.failed = true;
    }
    for link in &mut graph.links {
        let hit = match (link.src, link.dst) {
            (Endpoint::Spine(id), _) | (_, Endpoint::Spine(id)) => id == spine,
            _ => false,
        };
        if hit {
            mark_link_dead(link);
        }
    }
}

fn fail_rail(graph: &mut Graph, rail: RailId) {
    let leaves: Vec<LeafId> = graph
        .leaves
        .iter()
        .filter(|l| l.rail == rail)
        .map(|l| l.id)
        .collect();
    for leaf in leaves {
        fail_leaf(graph, leaf);
    }
}

fn fail_link(graph: &mut Graph, id: LinkId) {
    let (host, nic) = match graph.links.get_mut(id.0 as usize) {
        Some(link) if link.id == id => {
            let host = is_host_link(link);
            let nic = nic_of_host(link);
            mark_link_dead(link);
            (host, nic)
        }
        _ => return,
    };
    if host {
        if let Some(n) = nic {
            fail_gpu_nic(graph, GpuId(n.0));
        }
    }
}

fn apply_fail(graph: &mut Graph, p: &EventPayload) {
    match *p {
        EventPayload::SpineFail { spine } => fail_spine(graph, spine),
        EventPayload::LeafFail { leaf } => fail_leaf(graph, leaf),
        EventPayload::RailFail { rail } => fail_rail(graph, rail),
        EventPayload::LinkFail { link } => fail_link(graph, link),
        _ => {}
    }
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
            .gpu(GpuId(n.0))
            .map(|g| g.avail != GpuAvail::Present)
            .unwrap_or(true),
    }
}

fn path_hits_failed(graph: &Graph, path: &Path) -> bool {
    path.links.iter().any(|&e| {
        let Some(l) = graph.link(e) else {
            return true;
        };
        l.failed || endpoint_failed(graph, l.src) || endpoint_failed(graph, l.dst)
    })
}

fn job_on_dead_gpu(rec: &JobRec, graph: &Graph) -> bool {
    rec.binding.as_ref().is_some_and(|b| {
        b.map.iter().any(|(_, g)| {
            graph
                .gpu(*g)
                .map(|gpu| gpu.avail != GpuAvail::Present)
                .unwrap_or(true)
        })
    })
}

fn job_paths_hit_failed(rec: &JobRec, graph: &Graph) -> bool {
    rec.planned.iter().any(|f| path_hits_failed(graph, &f.path))
        || rec.paths.iter().any(|p| path_hits_failed(graph, p))
}

fn link_is_ls(graph: &Graph, id: LinkId) -> bool {
    graph.link(id).is_some_and(is_ls_link)
}

fn epoch_spine_class(graph: &Graph, fails: &[Event]) -> bool {
    fails.iter().any(|e| match e.payload {
        EventPayload::SpineFail { .. } => true,
        EventPayload::LinkFail { link } => link_is_ls(graph, link),
        _ => false,
    })
}

fn classify(rec: &JobRec, graph: &Graph, spine_class_epoch: bool) -> Class {
    if job_on_dead_gpu(rec, graph) {
        return Class::Kill;
    }
    if job_paths_hit_failed(rec, graph) {
        return Class::Spine;
    }
    if spine_class_epoch {
        Class::Spine
    } else {
        Class::Keep
    }
}

fn i2_broken(graph: &Graph, bytes: &[u64]) -> bool {
    graph.links.iter().any(|l| {
        if !l.failed {
            return false;
        }
        let b = bytes
            .get(l.id.0 as usize)
            .copied()
            .unwrap_or(l.bytes_this_epoch);
        b != 0 || l.bytes_this_epoch != 0
    })
}

fn i3_broken(graph: &Graph, table: &JobTable) -> bool {
    table
        .by_id
        .values()
        .filter(|r| live(r))
        .any(|r| job_paths_hit_failed(r, graph))
}

fn same_node(graph: &Graph, src: GpuId, dst: GpuId) -> bool {
    match (graph.gpu(src), graph.gpu(dst)) {
        (Some(s), Some(d)) => s.node == d.node,
        _ => false,
    }
}

fn inject_live_cir(residual: &mut Residual, graph: &Graph, rec: &JobRec) {
    for (&e, &rho) in &rec.cir {
        if rho > 0 && graph.link(e).is_some_and(|l| !l.failed) {
            residual.inject_cir(graph, e, rho);
        }
    }
}

fn try_reroute(
    rec: &JobRec,
    graph: &Graph,
    residual: &Residual,
    policy: Policy,
) -> Option<Reroute> {
    let binding = rec.binding.as_ref()?;
    match policy {
        Policy::Joint => {
            let f = evaluate(binding, &rec.spec, graph, residual).ok()?;
            Some(Reroute {
                job: rec.spec.id,
                paths: f.paths_chosen,
                planned: f.planned,
                cir: f.cir_add,
                t_pred_ps: f.t_pred,
            })
        }
        Policy::Naive => naive_rebuild(rec, graph, residual),
    }
}

fn naive_rebuild(rec: &JobRec, graph: &Graph, residual: &Residual) -> Option<Reroute> {
    let binding = rec.binding.as_ref()?;
    let job = &rec.spec;
    let comms = communicators(binding, job);
    let leftover = residual.physical_leftover(graph);
    let mut paths = Vec::new();
    let mut cir: BTreeMap<LinkId, u64> = BTreeMap::new();
    let mut t_pred: i128 = 0;
    let mut planned: Vec<Flow> = Vec::new();

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
                    .next()?;
                if path_hits_failed(graph, &path) {
                    return None;
                }
                paths.push(path.clone());
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
            let rates = water_fill(&flow_paths, &leftover).ok()?;
            let b_eff = rates.iter().copied().min().unwrap_or(0);
            if b_eff == 0 {
                return None;
            }
            t_comm = t_comm.saturating_add(phase_duration_ps(comm.chunk_bytes, b_eff));
            let mut load: BTreeMap<LinkId, u64> = BTreeMap::new();
            for (f, &rate) in flows.iter_mut().zip(rates.iter()) {
                f.rate_Bps = rate;
                for &e in &f.path.links {
                    *load.entry(e).or_insert(0) =
                        load.get(&e).copied().unwrap_or(0).saturating_add(rate);
                }
            }
            for (e, rho) in load {
                let slot = cir.entry(e).or_insert(0);
                if rho > *slot {
                    *slot = rho;
                }
            }
            planned.extend(flows);
        }
        if comm_has_fabric && t_comm > t_pred {
            t_pred = t_comm;
        }
    }
    if t_pred > job.deadline_ps {
        return None;
    }
    Some(Reroute {
        job: job.id,
        paths,
        planned,
        cir,
        t_pred_ps: t_pred,
    })
}

/// Pure prepare. No live Graph / Residual / JobTable mutation. §14.3
pub fn prepare(
    graph: &Graph,
    _residual: &Residual,
    table: &JobTable,
    fails: &[Event],
    policy: Policy,
    strict: bool,
    bytes_epoch: &[u64],
) -> Result<EpochPlan, RejectCode> {
    if strict && (i2_broken(graph, bytes_epoch) || i3_broken(graph, table)) {
        return Err(RejectCode::EpochPrepareFailed);
    }

    let from = graph.epoch;
    let mut graph_p = graph.clone();
    for e in fails {
        apply_fail(&mut graph_p, &e.payload);
    }
    for link in &mut graph_p.links {
        link.bytes_this_epoch = 0;
    }
    graph_p.epoch = EpochId(from.0.saturating_add(1));
    let to = graph_p.epoch;

    let spine_epoch = epoch_spine_class(&graph_p, fails);
    let mut kills = Vec::new();
    let mut spine_jobs = Vec::new();
    let mut keep = Vec::new();
    for id in live_ids(table) {
        let rec = table.by_id.get(&id).expect("live");
        match classify(rec, &graph_p, spine_epoch) {
            Class::Kill => kills.push(id),
            Class::Spine => spine_jobs.push(id),
            Class::Keep => keep.push(id),
        }
    }

    let mut residual_p = Residual::new(&graph_p);
    for id in &keep {
        if let Some(rec) = table.by_id.get(id) {
            inject_live_cir(&mut residual_p, &graph_p, rec);
        }
    }

    let mut reroutes = Vec::new();
    for id in spine_jobs {
        let rec = table.by_id.get(&id).expect("spine job");
        match try_reroute(rec, &graph_p, &residual_p, policy) {
            Some(r) => {
                for (&e, &rho) in &r.cir {
                    if rho > 0 {
                        residual_p.inject_cir(&graph_p, e, rho);
                    }
                }
                reroutes.push(r);
            }
            None => kills.push(id),
        }
    }
    kills.sort_by_key(|id| table.by_id.get(id).map(admit_key).unwrap_or(u64::MAX));

    Ok(EpochPlan {
        graph: graph_p,
        residual: residual_p,
        kills,
        reroutes,
        from,
        to,
    })
}

/// I2: failed ⇒ bytes_this_epoch == 0, capacity 0.
pub fn i2_holds(graph: &Graph, bytes_epoch: &[u64]) -> bool {
    !i2_broken(graph, bytes_epoch)
        && graph.links.iter().all(|l| {
            if !l.failed {
                return true;
            }
            l.capacity_Bps == 0
        })
}

pub fn i3_holds(graph: &Graph, table: &JobTable) -> bool {
    !i3_broken(graph, table)
}

/// Public so kernel can stamp Fail* payloads without duplicating the match.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::joint::joint_admit;
    use crate::kernel::{run_sim_snapshot, RunConfig};
    use crate::naive::naive_admit;
    use crate::table::JobTable;
    use fabric_model::{JobSpec, Mix};
    use fabric_sim::Fel;
    use fabric_types::{CollectiveKind, JobId, SimTime};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR: AtomicU64 = AtomicU64::new(0);

    fn n64() -> Graph {
        Graph::generate(512, 8, 1).expect("n64")
    }

    fn ring40() -> JobSpec {
        JobSpec {
            id: JobId(1),
            arrive: SimTime { ps: 0, seq: 0 },
            gpu_count: 40,
            dp: 40,
            tp: 1,
            pp: 1,
            collective: CollectiveKind::RingAllReduce,
            payload_bytes: 67_108_864,
            step_count: 20,
            compute_ps: 10_000_000_000,
            deadline_ps: 10_000_000_000,
        }
    }

    fn tiny2(id: u32) -> JobSpec {
        JobSpec {
            id: JobId(id),
            arrive: SimTime { ps: 0, seq: 0 },
            gpu_count: 2,
            dp: 2,
            tp: 1,
            pp: 1,
            collective: CollectiveKind::RingAllReduce,
            payload_bytes: 64,
            step_count: 4,
            compute_ps: 1_000_000_000,
            deadline_ps: 10_000_000_000,
        }
    }

    fn fail_ev(payload: EventPayload, seq: u64) -> Event {
        let kind = match payload {
            EventPayload::SpineFail { .. } => EventKind::SpineFail,
            EventPayload::LeafFail { .. } => EventKind::LeafFail,
            EventPayload::RailFail { .. } => EventKind::RailFail,
            EventPayload::LinkFail { .. } => EventKind::LinkFail,
            _ => EventKind::SpineFail,
        };
        Event {
            t: fabric_types::SimTime { ps: 1, seq },
            kind,
            payload,
        }
    }

    fn bytes_zero(g: &Graph) -> Vec<u64> {
        vec![0; g.links.len()]
    }

    fn out_dir() -> PathBuf {
        let n = TEST_DIR.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("fabric-pr9-{}-{}", std::process::id(), n))
    }

    #[test]
    fn parse_fail_default_is_1ps() {
        let f = parse_fail_spec("spine=3").expect("parse");
        assert_eq!(f.t_ps, 1);
        assert_eq!(f.kind, FailKind::Spine(SpineId(3)));
        let f = parse_fail_spec("spine=3@1s").expect("parse");
        assert_eq!(f.t_ps, 1_000_000_000_000);
    }

    #[test]
    fn fail_spine_reroute_or_kill() {
        let graph = n64();
        let mut residual = Residual::new(&graph);
        let mut table = JobTable::new();
        let mut fel = Fel::new();
        joint_admit(&ring40(), &graph, &mut residual, &mut table, &mut fel).expect("admit");
        assert_eq!(
            table.by_id.get(&JobId(1)).unwrap().state,
            JobState::Computing
        );

        let one = fail_ev(EventPayload::SpineFail { spine: SpineId(3) }, 0);
        let plan = prepare(
            &graph,
            &residual,
            &table,
            &[one],
            Policy::Joint,
            false,
            &bytes_zero(&graph),
        )
        .expect("prepare");
        assert!(
            plan.reroutes.iter().any(|r| r.job == JobId(1)),
            "path exists and T≤D_j ⇒ reroute; kills={:?} reroutes={:?}",
            plan.kills,
            plan.reroutes.iter().map(|r| r.job.0).collect::<Vec<_>>()
        );
        assert!(plan.kills.is_empty());
        assert_eq!(plan.to.0, 1);
        assert!(plan.graph.spines[3].failed);
        assert!(plan
            .graph
            .links
            .iter()
            .filter(|l| matches!(l.src, Endpoint::Spine(SpineId(3)))
                || matches!(l.dst, Endpoint::Spine(SpineId(3))))
            .all(|l| l.failed && l.capacity_Bps == 0 && l.bytes_this_epoch == 0));

        let all: Vec<Event> = (0..8)
            .map(|i| fail_ev(EventPayload::SpineFail { spine: SpineId(i) }, i as u64))
            .collect();
        let plan = prepare(
            &graph,
            &residual,
            &table,
            &all,
            Policy::Joint,
            false,
            &bytes_zero(&graph),
        )
        .expect("prepare all");
        assert!(
            plan.kills.contains(&JobId(1)),
            "no remaining path ⇒ kill; reroutes={:?}",
            plan.reroutes.iter().map(|r| r.job.0).collect::<Vec<_>>()
        );
        assert!(plan.reroutes.is_empty());
    }

    #[test]
    fn fail_leaf_kills_single_homed() {
        let graph = n64();
        let mut residual = Residual::new(&graph);
        let mut table = JobTable::new();
        let mut fel = Fel::new();
        naive_admit(&tiny2(1), &graph, &mut residual, &mut table, &mut fel).expect("j1");
        naive_admit(&tiny2(2), &graph, &mut residual, &mut table, &mut fel).expect("j2");
        let g1 = table
            .by_id
            .get(&JobId(1))
            .unwrap()
            .binding
            .as_ref()
            .unwrap()
            .map[0]
            .1;
        let leaf = graph.leaf_of(g1).expect("leaf");
        let j2_gpus: Vec<GpuId> = table
            .by_id
            .get(&JobId(2))
            .unwrap()
            .binding
            .as_ref()
            .unwrap()
            .map
            .iter()
            .map(|(_, g)| *g)
            .collect();
        assert!(j2_gpus.iter().all(|&g| graph.leaf_of(g) != Some(leaf)));

        let ev = fail_ev(EventPayload::LeafFail { leaf }, 0);
        let plan = prepare(
            &graph,
            &residual,
            &table,
            &[ev],
            Policy::Naive,
            false,
            &bytes_zero(&graph),
        )
        .expect("prepare");
        assert!(
            plan.kills.contains(&JobId(1)),
            "owner killed {:?}",
            plan.kills
        );
        assert!(
            !plan.kills.contains(&JobId(2)),
            "other job untouched kills={:?}",
            plan.kills
        );
        assert!(!plan.reroutes.iter().any(|r| r.job == JobId(2)));
    }

    #[test]
    fn fail_dead_zero_bytes() {
        let graph = n64();
        let mut residual = Residual::new(&graph);
        let mut table = JobTable::new();
        let mut fel = Fel::new();
        joint_admit(&ring40(), &graph, &mut residual, &mut table, &mut fel).expect("admit");
        let mut bytes = vec![99u64; graph.links.len()];
        let ev = fail_ev(EventPayload::SpineFail { spine: SpineId(3) }, 0);
        let plan = prepare(
            &graph,
            &residual,
            &table,
            &[ev],
            Policy::Joint,
            false,
            &bytes,
        )
        .expect("prepare");
        assert!(i2_holds(&plan.graph, &vec![0; plan.graph.links.len()]));
        for l in &plan.graph.links {
            if matches!(l.src, Endpoint::Spine(SpineId(3)))
                || matches!(l.dst, Endpoint::Spine(SpineId(3)))
            {
                assert!(l.failed);
                assert_eq!(l.capacity_Bps, 0);
                assert_eq!(l.bytes_this_epoch, 0);
            }
        }
        for (i, l) in plan.graph.links.iter().enumerate() {
            if l.failed {
                bytes[i] = 0;
            }
        }
        assert!(i2_holds(&plan.graph, &bytes));
    }

    #[test]
    fn epoch_2pc_arc_swap() {
        let graph = n64();
        let mix = Mix {
            seed: 1,
            horizon_ps: 200_000_000_000,
            jobs: vec![ring40()],
        };
        let out = out_dir();
        let snap = run_sim_snapshot(RunConfig {
            graph,
            mix,
            policy: Policy::Joint,
            seed: 1,
            out: out.clone(),
            strict: false,
            mix_hash: "t".into(),
            topo_hash: "t".into(),
            fails: vec![FailSpec {
                t_ps: 15_000_000_000,
                kind: FailKind::Spine(SpineId(3)),
            }],
            occupancy: crate::Occupancy::new(),
            residual: None,
        })
        .expect("run");
        let _ = std::fs::remove_dir_all(&out);
        assert_eq!(snap.epoch.0, 1, "EpochId increments once");
        let advances: Vec<_> = snap
            .event_trace
            .iter()
            .filter(|(k, _)| k == "EpochAdvance")
            .collect();
        assert_eq!(advances.len(), 1);
        assert_eq!(advances[0].1, 1);
        let idx = snap
            .event_trace
            .iter()
            .position(|(k, _)| k == "EpochAdvance")
            .expect("EpochAdvance");
        assert!(
            snap.event_trace[idx..].iter().all(|(_, e)| *e == 1),
            "post-commit events carry new id: {:?}",
            &snap.event_trace[idx..]
        );
        let pre = &snap.event_trace[..idx];
        assert!(
            pre.iter().any(|(k, e)| k == "SpineFail" && *e == 0),
            "Fail* traced in old epoch: {pre:?}"
        );
        assert_eq!(snap.graph.epoch.0, 1);
        assert!(snap.graph.spines[3].failed);
    }
}
