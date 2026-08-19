//! Job table, occupancy, communicators. Transcribed from docs/DESIGN.md §9.4, §8.1, §13.2.

use std::collections::{BTreeMap, VecDeque};

use fabric_model::JobSpec;
use fabric_sim::Path;
use fabric_topo::Graph;
use fabric_types::{
    AdmitSeq, BindingKind, CollectiveKind, FlowId, GpuAvail, GpuId, JobId, JobState, LinkId,
    NodeId, Rank, RejectCode,
};

#[derive(Clone, Debug)]
pub struct Binding {
    pub kind: BindingKind,
    pub map: Vec<(Rank, GpuId)>,
}

#[derive(Clone, Debug)]
pub struct Flow {
    pub id: FlowId,
    pub job: JobId,
    /// Communicator.index. Not in §9.4 Flow; needed to group phases at CollectiveStart.
    pub comm: u32,
    pub phase: u32,
    pub src: GpuId,
    pub dst: GpuId,
    pub path: Path,
    pub rate_Bps: u64,
    pub bytes: u64,
}

/// One generate_bindings candidate, for admit.jsonl / --explain. §13.6
#[derive(Clone, Debug)]
pub struct BindingNote {
    pub kind: BindingKind,
    pub gpu_ids: Vec<GpuId>,
    pub cost: f64,
    pub t_pred_ps: i128,
    pub code: Option<RejectCode>,
    pub phase0_links: Vec<LinkId>,
}

#[derive(Clone, Debug)]
pub struct JobRec {
    pub spec: JobSpec,
    pub state: JobState,
    pub admit_seq: Option<AdmitSeq>,
    pub binding: Option<Binding>,
    pub paths: Vec<Path>,
    pub cir: BTreeMap<LinkId, u64>,
    /// Admit-time fabric flows with CIR rates. Updated on RateRecompute replay.
    pub planned: Vec<Flow>,
    pub t_pred_ps: i128,
    pub step_index: u32,
    pub steps_done: u32,
    pub reject: Option<RejectCode>,
    /// Joint: every K≤16 candidate. Naive: empty (write_admit synthesizes one).
    pub notes: Vec<BindingNote>,
    pub chosen_idx: Option<usize>,
}

pub struct Occupancy {
    pub by_gpu: BTreeMap<GpuId, JobId>,
}

impl Occupancy {
    pub fn new() -> Self {
        Self {
            by_gpu: BTreeMap::new(),
        }
    }

    pub fn is_free(&self, g: GpuId, graph: &Graph) -> bool {
        graph
            .gpu(g)
            .is_some_and(|gpu| gpu.avail == GpuAvail::Present)
            && !self.by_gpu.contains_key(&g)
    }
}

impl Default for Occupancy {
    fn default() -> Self {
        Self::new()
    }
}

pub struct JobTable {
    pub by_id: BTreeMap<JobId, JobRec>,
    pub next_admit_seq: u64,
    pub occ: Occupancy,
}

impl JobTable {
    pub fn new() -> Self {
        Self {
            by_id: BTreeMap::new(),
            next_admit_seq: 0,
            occ: Occupancy::new(),
        }
    }
}

impl Default for JobTable {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Communicator {
    pub index: u32,
    pub kind: CollectiveKind,
    pub members: Vec<GpuId>,
    pub p: u32,
    pub chunk_bytes: u64,
    pub n_phases: u32,
}

impl Communicator {
    pub fn edges(&self, phase: u32) -> Vec<(GpuId, GpuId)> {
        if self.p < 2 {
            return Vec::new();
        }
        let p = self.p as usize;
        match self.kind {
            CollectiveKind::RingAllReduce => (0..p)
                .map(|i| (self.members[i], self.members[(i + 1) % p]))
                .collect(),
            CollectiveKind::PairwiseAllToAll => {
                let h = (phase as usize) + 1;
                (0..p)
                    .map(|i| (self.members[i], self.members[(i + h) % p]))
                    .collect()
            }
        }
    }
}

/// Bucket by NodeId, sort by local rank (rail). Walk pp, then dp, then tp.
pub fn rank_map(gpus: &[GpuId], job: &JobSpec, graph: &Graph) -> Vec<(Rank, GpuId)> {
    let mut buckets: BTreeMap<NodeId, VecDeque<GpuId>> = BTreeMap::new();
    for &g in gpus {
        let Some(gpu) = graph.gpu(g) else {
            continue;
        };
        buckets.entry(gpu.node).or_default().push_back(g);
    }
    for q in buckets.values_mut() {
        let mut v: Vec<GpuId> = q.drain(..).collect();
        v.sort_by_key(|&g| graph.gpu(g).map(|gpu| gpu.rail.0).unwrap_or(u8::MAX));
        *q = v.into();
    }

    let mut map = Vec::with_capacity(job.gpu_count as usize);
    for pp_idx in 0..job.pp {
        for dp_idx in 0..job.dp {
            for tp_idx in 0..job.tp {
                let rank = Rank(pp_idx * (job.dp * job.tp) + dp_idx * job.tp + tp_idx);
                let gpu = buckets
                    .values_mut()
                    .find_map(|q| q.pop_front())
                    .expect("rank_map: picked set smaller than gpu_count");
                map.push((rank, gpu));
            }
        }
    }
    map
}

pub fn communicators(binding: &Binding, job: &JobSpec) -> Vec<Communicator> {
    let p = job.dp;
    if p == 0 {
        return Vec::new();
    }
    let mut by_rank = vec![GpuId(0); job.gpu_count as usize];
    for &(Rank(r), g) in &binding.map {
        if let Some(slot) = by_rank.get_mut(r as usize) {
            *slot = g;
        }
    }
    let chunk_bytes = job.payload_bytes / u64::from(p);
    let n_phases = match job.collective {
        CollectiveKind::RingAllReduce => 2 * p.saturating_sub(1),
        CollectiveKind::PairwiseAllToAll => p.saturating_sub(1),
    };
    let mut out = Vec::new();
    let mut index = 0u32;
    for pp_idx in 0..job.pp {
        for tp_idx in 0..job.tp {
            let mut members = Vec::with_capacity(p as usize);
            for dp_idx in 0..p {
                let rank = pp_idx * (job.dp * job.tp) + dp_idx * job.tp + tp_idx;
                members.push(by_rank[rank as usize]);
            }
            out.push(Communicator {
                index,
                kind: job.collective,
                members,
                p,
                chunk_bytes,
                n_phases,
            });
            index += 1;
        }
    }
    out
}
