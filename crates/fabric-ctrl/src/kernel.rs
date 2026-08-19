//! Naive DES event loop. Transcribed from docs/DESIGN.md §10, §9.7–§9.8, §16.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use fabric_model::Mix;
use fabric_report::{tail_p99_ps, us_of, Counts, JobRow, Metrics, Report, TopoSummary};
use fabric_sim::{
    phase_duration_ps, water_fill, Event, EventKind, EventPayload, Fel, Path, Residual,
};
use fabric_topo::Graph;
use fabric_trace::{ps_i64, EventRow, FlowRow, JobRow as TraceJob, LinkRow, TraceError, TraceSink};
use fabric_types::{
    EpochId, FlowId, GpuId, JobId, JobState, LinkId, Policy, ProcessExit, RecomputeReason,
    RejectCode,
};
use serde_json::json;

use crate::epoch::{prepare, EpochPlan, FailSpec};
use crate::joint::joint_admit;
use crate::naive::naive_admit;
use crate::table::{BindingNote, Flow as PlannedFlow, JobTable};

pub struct RunConfig {
    pub graph: Graph,
    pub mix: Mix,
    pub policy: Policy,
    pub seed: u64,
    pub out: PathBuf,
    pub strict: bool,
    pub mix_hash: String,
    pub topo_hash: String,
    pub fails: Vec<FailSpec>,
}

/// Post-run snapshot for PR9 tests (not serialized).
pub struct RunSnapshot {
    pub report: Report,
    pub epoch: EpochId,
    pub event_trace: Vec<(String, u32)>,
    pub bytes_epoch: Vec<u64>,
    pub graph: Graph,
}

#[derive(Debug)]
pub enum RunError {
    Usage(String),
    Io(String),
    Inv(String),
}

impl RunError {
    pub fn exit(&self) -> ProcessExit {
        match self {
            RunError::Usage(_) => ProcessExit::Usage,
            RunError::Io(_) => ProcessExit::IoAbort,
            RunError::Inv(_) => ProcessExit::InvariantFail,
        }
    }

    pub fn e_code(&self) -> &'static str {
        match self {
            RunError::Usage(_) => "E_USAGE",
            RunError::Io(_) => "E_IO",
            RunError::Inv(_) => "E_INV",
        }
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            RunError::Usage(m) | RunError::Io(m) | RunError::Inv(m) => m.as_str(),
        };
        write!(f, "error[{}]: {msg}", self.e_code())
    }
}

impl From<TraceError> for RunError {
    fn from(e: TraceError) -> Self {
        RunError::Io(e.0)
    }
}

struct LiveFlow {
    job: JobId,
    phase: u32,
    src: GpuId,
    dst: GpuId,
    path: Path,
    rate_Bps: u64,
    bytes: u64,
    t_arrive_ps: i128,
    t_depart_ps: i128,
}

struct Kernel {
    graph: Arc<Graph>,
    residual: Residual,
    table: JobTable,
    fel: Fel,
    traces: TraceSink,
    specs: BTreeMap<JobId, fabric_model::JobSpec>,
    /// Flows scheduled but not yet departed.
    flows: BTreeMap<FlowId, LiveFlow>,
    /// Currently transmitting (after FlowArrive, before FlowDepart).
    inflight: BTreeMap<FlowId, ()>,
    next_flow: u64,
    now: i128,
    recompute_at: Option<i128>,
    need_cir_replay: bool,
    strict: bool,
    q_bytes: Vec<u64>,
    #[allow(dead_code)]
    overflowed: Vec<bool>,
    #[allow(dead_code)]
    bytes_epoch: Vec<u64>,
    rate_dt: Vec<i128>,
    hotspot_ps: i128,
    last_periodic_ps: i128,
    snap_period_ps: i128,
    disrupted_ps: i128,
    invariants_ok: bool,
    collective_start: BTreeMap<JobId, i128>,
    collective_durs: Vec<i128>,
    job_slo_ok: BTreeMap<JobId, bool>,
    job_exit: BTreeMap<JobId, i128>,
    job_slo_miss_ps: i128,
    counts: Counts,
    rejects_by_code: BTreeMap<String, u64>,
    seed: u64,
    policy: Policy,
    mix_hash: String,
    topo_hash: String,
    horizon_ps: i128,
    admit_frozen: bool,
    pending_end_seq: BTreeMap<JobId, u64>,
    fail_log: Vec<serde_json::Value>,
    event_trace: Vec<(String, u32)>,
}

pub fn run_sim(cfg: RunConfig) -> Result<Report, RunError> {
    Ok(run_sim_snapshot(cfg)?.report)
}

pub fn run_sim_snapshot(cfg: RunConfig) -> Result<RunSnapshot, RunError> {
    let g_tot = cfg.graph.gpus.len() as u32;
    let snap_period_ps = if g_tot > 2048 {
        10_000_000_000 // 10 ms
    } else {
        1_000_000_000 // 1 ms
    };
    let nlink = cfg.graph.links.len();
    let traces = TraceSink::create(&cfg.out, cfg.seed)?;
    let mut specs = BTreeMap::new();
    for j in &cfg.mix.jobs {
        specs.insert(j.id, j.clone());
    }
    let graph = Arc::new(cfg.graph);
    let mut k = Kernel {
        residual: Residual::new(&graph),
        q_bytes: vec![0; nlink],
        overflowed: vec![false; nlink],
        bytes_epoch: vec![0; nlink],
        rate_dt: vec![0; nlink],
        graph,
        table: JobTable::new(),
        fel: Fel::new(),
        traces,
        specs,
        flows: BTreeMap::new(),
        inflight: BTreeMap::new(),
        next_flow: 0,
        now: 0,
        recompute_at: None,
        need_cir_replay: false,
        strict: cfg.strict,
        hotspot_ps: 0,
        last_periodic_ps: 0,
        snap_period_ps,
        disrupted_ps: 0,
        invariants_ok: true,
        collective_start: BTreeMap::new(),
        collective_durs: Vec::new(),
        job_slo_ok: BTreeMap::new(),
        job_exit: BTreeMap::new(),
        job_slo_miss_ps: 0,
        counts: Counts::default(),
        rejects_by_code: Report::empty_rejects(),
        seed: cfg.seed,
        policy: cfg.policy,
        mix_hash: cfg.mix_hash,
        topo_hash: cfg.topo_hash,
        horizon_ps: cfg.mix.horizon_ps,
        admit_frozen: false,
        pending_end_seq: BTreeMap::new(),
        fail_log: Vec::new(),
        event_trace: Vec::new(),
    };
    for j in &cfg.mix.jobs {
        k.fel.push(
            j.arrive.ps,
            EventKind::JobArrive,
            EventPayload::JobArrive { job: j.id },
        );
    }
    for f in &cfg.fails {
        let (kind, payload) = f.event();
        k.fel.push(f.t_ps, kind, payload);
    }
    k.fel.push(
        cfg.mix.horizon_ps,
        EventKind::HorizonCut,
        EventPayload::HorizonCut,
    );

    while let Some(ev) = k.fel.pop() {
        k.advance(ev.t.ps);
        k.trace_event(&ev);
        let stop = k.handle(&ev)?;
        if stop {
            break;
        }
    }
    k.finish()
}

impl Kernel {
    fn advance(&mut self, now: i128) {
        if now < self.now {
            return;
        }
        if now > self.now {
            let dt = now - self.now;
            let load = self.inflight_load();
            let buf = self.graph.params.buffer_bytes as i128;
            let inf = self.graph.params.buffer_infinite;
            for (i, link) in self.graph.links.iter().enumerate() {
                let c = link.capacity_Bps;
                let r = load[i];
                if c > 0 && r.saturating_mul(5) >= c.saturating_mul(4) {
                    self.hotspot_ps = self.hotspot_ps.saturating_add(dt);
                }
                self.rate_dt[i] = self.rate_dt[i].saturating_add((r as i128).saturating_mul(dt));
                let dq = if r >= c {
                    ((r - c) as i128).saturating_mul(dt) / 1_000_000_000_000
                } else {
                    -((c - r) as i128).saturating_mul(dt) / 1_000_000_000_000
                };
                let q = self.q_bytes[i] as i128 + dq;
                let q = if inf { q.max(0) } else { q.clamp(0, buf) };
                if !inf && q == buf && dq > 0 {
                    self.overflowed[i] = true;
                }
                self.q_bytes[i] = q as u64;
            }
            self.maybe_periodic(now, &load);
        }
        self.now = now;
    }

    fn maybe_periodic(&mut self, now: i128, load: &[u64]) {
        if self.inflight.is_empty() {
            return;
        }
        let mut t = self.last_periodic_ps + self.snap_period_ps;
        while t <= now {
            self.snapshot_active_links(t, load);
            self.last_periodic_ps = t;
            t = t.saturating_add(self.snap_period_ps);
            if t <= self.last_periodic_ps {
                break;
            }
        }
    }

    fn snapshot_active_links(&mut self, t: i128, load: &[u64]) {
        let t_ps = ps_i64(t);
        for (i, link) in self.graph.links.iter().enumerate() {
            let cir = self.residual.cir.get(i).copied().unwrap_or(0);
            let live = load.get(i).copied().unwrap_or(0);
            if cir == 0 && live == 0 && self.q_bytes[i] == 0 {
                continue;
            }
            self.traces.link(LinkRow {
                link_id: link.id.0,
                t_ps,
                c_Bps: link.capacity_Bps,
                cir_Bps: cir,
                r_avail_Bps: self.residual.r_avail.get(i).copied().unwrap_or(0),
                q_bytes: self.q_bytes[i],
                failed: link.failed,
            });
        }
    }

    fn snapshot_all_links(&mut self, t: i128) {
        let t_ps = ps_i64(t);
        for (i, link) in self.graph.links.iter().enumerate() {
            self.traces.link(LinkRow {
                link_id: link.id.0,
                t_ps,
                c_Bps: link.capacity_Bps,
                cir_Bps: self.residual.cir.get(i).copied().unwrap_or(0),
                r_avail_Bps: self.residual.r_avail.get(i).copied().unwrap_or(0),
                q_bytes: self.q_bytes[i],
                failed: link.failed,
            });
        }
    }

    fn trace_event(&mut self, e: &Event) {
        let (job_id, flow_id, link_id, spine_id, leaf_id, rail_id) = match e.payload {
            EventPayload::JobArrive { job }
            | EventPayload::StepBoundary { job, .. }
            | EventPayload::CollectiveStart { job, .. }
            | EventPayload::CollectiveEnd { job, .. }
            | EventPayload::DrainComplete { job } => (Some(job.0), None, None, None, None, None),
            EventPayload::FlowArrive { flow } | EventPayload::FlowDepart { flow } => {
                let job = self.flows.get(&flow).map(|f| f.job.0);
                (job, Some(flow.0), None, None, None, None)
            }
            EventPayload::RateRecompute { reason } => {
                let j = match reason {
                    RecomputeReason::Admit(j) | RecomputeReason::JobExit(j) => Some(j.0),
                    RecomputeReason::EpochCommit => None,
                };
                (j, None, None, None, None, None)
            }
            EventPayload::LinkFail { link } => (None, None, Some(link.0), None, None, None),
            EventPayload::LeafFail { leaf } => (None, None, None, None, Some(leaf.0), None),
            EventPayload::RailFail { rail } => (None, None, None, None, None, Some(rail.0)),
            EventPayload::SpineFail { spine } => (None, None, None, Some(spine.0), None, None),
            EventPayload::EpochAdvance { .. } | EventPayload::HorizonCut => {
                (None, None, None, None, None, None)
            }
        };
        self.event_trace
            .push((e.kind.as_str().to_string(), self.graph.epoch.0));
        self.traces.event(EventRow {
            t_ps: ps_i64(e.t.ps),
            seq: e.t.seq,
            kind: e.kind.as_str().to_string(),
            epoch: self.graph.epoch.0,
            job_id,
            flow_id,
            link_id,
            spine_id,
            leaf_id,
            rail_id,
            reject: None,
            bytes: None,
        });
    }

    fn handle(&mut self, e: &Event) -> Result<bool, RunError> {
        match e.payload {
            EventPayload::JobArrive { job } => self.on_arrive(job)?,
            EventPayload::StepBoundary { job, step } => self.on_step(job, step, e.t.ps),
            EventPayload::CollectiveStart { job, step } => {
                self.on_collective_start(job, step, e.t.ps)
            }
            EventPayload::CollectiveEnd { job, step } => {
                self.on_collective_end(job, step, e.t.ps, e.t.seq)?
            }
            EventPayload::FlowArrive { flow } => self.on_flow_arrive(flow)?,
            EventPayload::FlowDepart { flow } => self.on_flow_depart(flow, e.t.ps),
            EventPayload::RateRecompute { .. } => self.on_recompute(e.t.ps)?,
            EventPayload::HorizonCut => {
                self.on_horizon(e.t.ps)?;
                return Ok(true);
            }
            EventPayload::LinkFail { .. }
            | EventPayload::LeafFail { .. }
            | EventPayload::RailFail { .. }
            | EventPayload::SpineFail { .. } => self.on_fail_star(e)?,
            EventPayload::DrainComplete { .. } | EventPayload::EpochAdvance { .. } => {}
        }
        Ok(false)
    }

    fn on_arrive(&mut self, job: JobId) -> Result<(), RunError> {
        if self.admit_frozen {
            self.fel.push(
                self.now,
                EventKind::JobArrive,
                EventPayload::JobArrive { job },
            );
            return Ok(());
        }
        self.counts.arrivals += 1;
        let spec = self
            .specs
            .get(&job)
            .cloned()
            .ok_or_else(|| RunError::Io(format!("unknown job {}", job.0)))?;
        let free = self
            .graph
            .gpus
            .iter()
            .filter(|g| self.table.occ.is_free(g.id, &self.graph))
            .count();
        let admit = match self.policy {
            Policy::Naive => naive_admit(
                &spec,
                &self.graph,
                &mut self.residual,
                &mut self.table,
                &mut self.fel,
            ),
            Policy::Joint => joint_admit(
                &spec,
                &self.graph,
                &mut self.residual,
                &mut self.table,
                &mut self.fel,
            ),
        };
        match admit {
            Ok(()) => {
                self.counts.admits += 1;
                self.job_slo_ok.insert(job, true);
                self.write_admit(&spec, free, true, None)?;
                self.queue_recompute(self.now, RecomputeReason::Admit(job));
            }
            Err(code) => {
                self.counts.rejects += 1;
                *self
                    .rejects_by_code
                    .entry(code.as_str().to_string())
                    .or_insert(0) += 1;
                self.write_admit(&spec, free, false, Some(code))?;
                self.write_job_trace(job, "reject", self.now);
            }
        }
        Ok(())
    }

    fn on_step(&mut self, job: JobId, step: u32, now: i128) {
        let Some(rec) = self.table.by_id.get_mut(&job) else {
            return;
        };
        if rec.state == JobState::Killed {
            return;
        }
        rec.state = JobState::Collecting;
        rec.step_index = step;
        self.fel.push(
            now,
            EventKind::CollectiveStart,
            EventPayload::CollectiveStart { job, step },
        );
    }

    fn on_collective_start(&mut self, job: JobId, step: u32, now: i128) {
        let Some(rec) = self.table.by_id.get(&job) else {
            return;
        };
        if rec.state == JobState::Killed || rec.state == JobState::Rejected {
            return;
        }
        self.collective_start.insert(job, now);
        let planned = rec.planned.clone();
        if planned.is_empty() {
            self.fel.push(
                now,
                EventKind::CollectiveEnd,
                EventPayload::CollectiveEnd { job, step },
            );
            return;
        }
        let mut keys: Vec<(u32, u32)> = planned.iter().map(|f| (f.comm, f.phase)).collect();
        keys.sort_unstable();
        keys.dedup();
        let mut max_end = now;
        let mut any = false;
        for &(comm, phase) in &keys {
            let group: Vec<&PlannedFlow> = planned
                .iter()
                .filter(|f| f.comm == comm && f.phase == phase)
                .collect();
            if group.is_empty() {
                continue;
            }
            let b_eff = group.iter().map(|f| f.rate_Bps).min().unwrap_or(0);
            let chunk = group[0].bytes;
            let d = phase_duration_ps(chunk, b_eff);
            if d == i128::MAX {
                continue;
            }
            any = true;
            // Phase start: max_end tracks per-comm serial time. Parallel comms share `now`.
            let _ = comm;
            let t0 = now.saturating_add(phase_offset(&planned, comm, phase, now));
            let t1 = t0.saturating_add(d);
            if t1 > max_end {
                max_end = t1;
            }
            for f in group {
                let id = FlowId(self.next_flow);
                self.next_flow += 1;
                self.flows.insert(
                    id,
                    LiveFlow {
                        job,
                        phase,
                        src: f.src,
                        dst: f.dst,
                        path: f.path.clone(),
                        rate_Bps: f.rate_Bps,
                        bytes: f.bytes,
                        t_arrive_ps: t0,
                        t_depart_ps: t1,
                    },
                );
                self.fel.push(
                    t0,
                    EventKind::FlowArrive,
                    EventPayload::FlowArrive { flow: id },
                );
                self.fel.push(
                    t1,
                    EventKind::FlowDepart,
                    EventPayload::FlowDepart { flow: id },
                );
            }
        }
        if !any {
            self.fel.push(
                now,
                EventKind::CollectiveEnd,
                EventPayload::CollectiveEnd { job, step },
            );
        } else {
            self.fel.push(
                max_end,
                EventKind::CollectiveEnd,
                EventPayload::CollectiveEnd { job, step },
            );
        }
        self.pending_end_seq.insert(job, self.fel.last_push_seq());
    }

    fn on_collective_end(
        &mut self,
        job: JobId,
        step: u32,
        now: i128,
        seq: u64,
    ) -> Result<(), RunError> {
        if self.pending_end_seq.get(&job) != Some(&seq) {
            return Ok(());
        }
        let t0 = self.collective_start.remove(&job).unwrap_or(now);
        let dur = now.saturating_sub(t0);
        let (more, compute, d_j, killed) = {
            let Some(rec) = self.table.by_id.get(&job) else {
                return Ok(());
            };
            if rec.state == JobState::Killed {
                return Ok(());
            }
            (
                rec.steps_done + 1 < rec.spec.step_count,
                rec.spec.compute_ps,
                rec.spec.deadline_ps,
                false,
            )
        };
        let _ = killed;
        if let Some(rec) = self.table.by_id.get_mut(&job) {
            rec.steps_done = rec.steps_done.saturating_add(1);
            if more {
                rec.state = JobState::Computing;
                rec.step_index = step.saturating_add(1);
            } else {
                rec.state = JobState::Completed;
            }
        }
        if dur > d_j {
            self.job_slo_ok.insert(job, false);
            self.job_slo_miss_ps = self.job_slo_miss_ps.saturating_add(dur - d_j);
        }
        self.collective_durs.push(dur);
        if more {
            self.fel.push(
                now.saturating_add(compute),
                EventKind::StepBoundary,
                EventPayload::StepBoundary {
                    job,
                    step: step.saturating_add(1),
                },
            );
        } else {
            self.job_exit.insert(job, now);
            self.counts.completes += 1;
            self.release_job(job);
            self.write_job_trace(job, "admit", now);
            self.need_cir_replay = true;
            self.queue_recompute(now, RecomputeReason::JobExit(job));
        }
        Ok(())
    }

    fn on_flow_arrive(&mut self, flow: FlowId) -> Result<(), RunError> {
        if self.flows.contains_key(&flow) {
            self.inflight.insert(flow, ());
        }
        self.check_i1()?;
        Ok(())
    }

    fn on_flow_depart(&mut self, flow: FlowId, now: i128) {
        self.inflight.remove(&flow);
        let Some(f) = self.flows.remove(&flow) else {
            return;
        };
        for &e in &f.path.links {
            let i = e.0 as usize;
            if i < self.bytes_epoch.len() && self.graph.links.get(i).is_some_and(|l| !l.failed) {
                self.bytes_epoch[i] = self.bytes_epoch[i].saturating_add(f.bytes);
            }
        }
        self.traces.flow(FlowRow {
            flow_id: flow.0,
            job_id: f.job.0,
            phase: f.phase,
            src_gpu: f.src.0,
            dst_gpu: f.dst.0,
            path_links: f.path.links.iter().map(|l| l.0).collect(),
            rate_Bps: f.rate_Bps,
            bytes: f.bytes,
            t_arrive_ps: ps_i64(f.t_arrive_ps),
            t_depart_ps: ps_i64(f.t_depart_ps.max(now)),
        });
    }

    fn on_recompute(&mut self, now: i128) -> Result<(), RunError> {
        self.recompute_at = None;
        if self.need_cir_replay {
            self.replay_cir();
            self.need_cir_replay = false;
        }
        self.recompute_realized();
        self.check_i1()?;
        self.snapshot_all_links(now);
        Ok(())
    }

    fn on_horizon(&mut self, now: i128) -> Result<(), RunError> {
        let live: Vec<JobId> = self
            .table
            .by_id
            .iter()
            .filter(|(_, r)| matches!(r.state, JobState::Computing | JobState::Collecting))
            .map(|(id, _)| *id)
            .collect();
        for id in live {
            let state = self.table.by_id.get(&id).map(|r| r.state);
            match state {
                Some(JobState::Computing) => {}
                Some(JobState::Collecting) => {
                    if let Some(t0) = self.collective_start.get(&id) {
                        self.disrupted_ps = self.disrupted_ps.saturating_add(now - t0);
                    }
                }
                _ => continue,
            }
            if let Some(rec) = self.table.by_id.get_mut(&id) {
                rec.state = JobState::Killed;
            }
            let drop: Vec<FlowId> = self
                .flows
                .iter()
                .filter(|(_, f)| f.job == id)
                .map(|(fid, _)| *fid)
                .collect();
            for fid in drop {
                self.inflight.remove(&fid);
                self.flows.remove(&fid);
            }
            self.drop_occ(id);
            self.job_exit.insert(id, now);
            self.write_job_trace(id, "kill", now);
            self.counts.kills += 1;
            self.need_cir_replay = true;
        }
        if self.need_cir_replay {
            self.replay_cir();
            self.need_cir_replay = false;
        }
        self.recompute_realized();
        self.snapshot_all_links(now);
        Ok(())
    }

    fn on_fail_star(&mut self, first: &Event) -> Result<(), RunError> {
        self.admit_frozen = true;
        let mut fails = vec![first.clone()];
        fails.extend(self.fel.drain_fails_at(first.t.ps));
        for extra in fails.iter().skip(1) {
            self.trace_event(extra);
        }
        let plan = prepare(
            &self.graph,
            &self.residual,
            &self.table,
            &fails,
            self.policy,
            self.strict,
            &self.bytes_epoch,
        )
        .map_err(|code| RunError::Inv(code.as_str().into()))?;
        self.commit(plan, first.t.ps)?;
        Ok(())
    }

    fn commit(&mut self, plan: EpochPlan, now: i128) -> Result<(), RunError> {
        self.graph = Arc::new(plan.graph);
        self.residual = plan.residual;
        self.bytes_epoch.fill(0);

        let kills = plan.kills.clone();
        let reroutes = plan.reroutes.clone();
        for id in &kills {
            if let Some(rec) = self.table.by_id.get_mut(id) {
                rec.state = JobState::Killed;
            }
            self.abort_job_flows(*id, now, false);
            self.drop_occ(*id);
            self.job_exit.insert(*id, now);
            self.write_job_trace(*id, "kill", now);
            self.counts.kills += 1;
        }
        for r in &reroutes {
            if let Some(rec) = self.table.by_id.get_mut(&r.job) {
                rec.paths = r.paths.clone();
                rec.planned = r.planned.clone();
                rec.cir = r.cir.clone();
                rec.t_pred_ps = r.t_pred_ps;
            }
            let collecting = self
                .table
                .by_id
                .get(&r.job)
                .is_some_and(|rec| rec.state == JobState::Collecting);
            if collecting && self.collective_start.contains_key(&r.job) {
                if let Some(t0) = self.collective_start.get(&r.job).copied() {
                    self.disrupted_ps = self.disrupted_ps.saturating_add(now.saturating_sub(t0));
                }
                self.abort_job_flows(r.job, now, true);
                let step = self
                    .table
                    .by_id
                    .get(&r.job)
                    .map(|rec| rec.step_index)
                    .unwrap_or(0);
                self.fel.push(
                    now,
                    EventKind::CollectiveStart,
                    EventPayload::CollectiveStart { job: r.job, step },
                );
            }
        }

        self.need_cir_replay = true;
        self.queue_recompute(now, RecomputeReason::EpochCommit);
        for id in &kills {
            self.fel.push(
                now,
                EventKind::DrainComplete,
                EventPayload::DrainComplete { job: *id },
            );
        }
        self.fel.push(
            now,
            EventKind::EpochAdvance,
            EventPayload::EpochAdvance {
                from: plan.from,
                to: plan.to,
            },
        );

        let dead_bytes: u64 = self
            .graph
            .links
            .iter()
            .enumerate()
            .filter(|(_, l)| l.failed)
            .map(|(i, _)| self.bytes_epoch.get(i).copied().unwrap_or(0))
            .sum();
        self.fail_log.push(json!({
            "t_ps": now as i64,
            "kills": kills.iter().map(|j| j.0).collect::<Vec<_>>(),
            "reroutes": reroutes.iter().map(|r| r.job.0).collect::<Vec<_>>(),
            "epoch_from": plan.from.0,
            "epoch_to": plan.to.0,
            "dead_link_bytes": dead_bytes,
        }));
        self.check_i2()?;
        if !crate::epoch::i3_holds(&self.graph, &self.table) {
            self.invariants_ok = false;
            if self.strict {
                return Err(RunError::Inv("I3".into()));
            }
        }
        self.admit_frozen = false;
        Ok(())
    }

    fn abort_job_flows(&mut self, job: JobId, now: i128, emit_depart: bool) {
        let ids: Vec<FlowId> = self
            .flows
            .iter()
            .filter(|(_, f)| f.job == job)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if emit_depart && self.inflight.contains_key(&id) {
                if let Some(f) = self.flows.get_mut(&id) {
                    let elapsed = now.saturating_sub(f.t_arrive_ps).max(0);
                    let moved =
                        ((f.rate_Bps as i128).saturating_mul(elapsed) / 1_000_000_000_000) as u64;
                    f.bytes = moved.min(f.bytes);
                    f.t_depart_ps = now;
                }
                self.on_flow_depart(id, now);
            } else {
                self.inflight.remove(&id);
                self.flows.remove(&id);
            }
        }
    }

    fn check_i2(&mut self) -> Result<(), RunError> {
        if !crate::epoch::i2_holds(&self.graph, &self.bytes_epoch) {
            self.invariants_ok = false;
            if self.strict {
                return Err(RunError::Inv("I2".into()));
            }
        }
        Ok(())
    }

    fn queue_recompute(&mut self, ps: i128, reason: RecomputeReason) {
        if self.recompute_at == Some(ps) {
            return;
        }
        self.recompute_at = Some(ps);
        self.fel.push(
            ps,
            EventKind::RateRecompute,
            EventPayload::RateRecompute { reason },
        );
    }

    fn drop_occ(&mut self, id: JobId) {
        let gpus: Vec<GpuId> = self
            .table
            .by_id
            .get(&id)
            .and_then(|rec| rec.binding.as_ref())
            .map(|b| b.map.iter().map(|(_, g)| *g).collect())
            .unwrap_or_default();
        for g in gpus {
            self.table.occ.by_gpu.remove(&g);
        }
    }

    fn release_job(&mut self, id: JobId) {
        let cir = {
            let Some(rec) = self.table.by_id.get(&id) else {
                return;
            };
            rec.cir.clone()
        };
        for (&e, &rho) in &cir {
            self.residual.release_cir(&self.graph, e, rho);
        }
        self.drop_occ(id);
    }

    fn replay_cir(&mut self) {
        self.residual.clear_cir(&self.graph);
        let mut ids: Vec<JobId> = self
            .table
            .by_id
            .iter()
            .filter(|(_, r)| matches!(r.state, JobState::Computing | JobState::Collecting))
            .map(|(id, _)| *id)
            .collect();
        ids.sort_by_key(|id| {
            self.table
                .by_id
                .get(id)
                .and_then(|r| r.admit_seq)
                .map(|s| s.0)
                .unwrap_or(u64::MAX)
        });
        for id in ids {
            self.fill_job_cir(id);
        }
    }

    fn fill_job_cir(&mut self, id: JobId) {
        let leftover = match self.policy {
            Policy::Joint => self.residual.r_avail.clone(),
            Policy::Naive => self.residual.physical_leftover(&self.graph),
        };
        let Some(rec) = self.table.by_id.get_mut(&id) else {
            return;
        };
        let mut keys: Vec<(u32, u32)> = rec.planned.iter().map(|f| (f.comm, f.phase)).collect();
        keys.sort_unstable();
        keys.dedup();
        let mut cir: BTreeMap<LinkId, u64> = BTreeMap::new();
        for &(comm, phase) in &keys {
            let idxs: Vec<usize> = rec
                .planned
                .iter()
                .enumerate()
                .filter(|(_, f)| f.comm == comm && f.phase == phase)
                .map(|(i, _)| i)
                .collect();
            if idxs.is_empty() {
                continue;
            }
            let paths: Vec<Path> = idxs.iter().map(|&i| rec.planned[i].path.clone()).collect();
            match water_fill(&paths, &leftover) {
                Ok(rates) => {
                    for (&i, &rate) in idxs.iter().zip(rates.iter()) {
                        rec.planned[i].rate_Bps = rate;
                        for &e in &rec.planned[i].path.links {
                            let slot = cir.entry(e).or_insert(0);
                            *slot = (*slot).max(rate); // per-flow; accumulate below
                        }
                    }
                    let mut load: BTreeMap<LinkId, u64> = BTreeMap::new();
                    for (&i, &rate) in idxs.iter().zip(rates.iter()) {
                        for &e in &rec.planned[i].path.links {
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
                }
                Err(_) => {
                    for i in idxs {
                        rec.planned[i].rate_Bps = 0;
                    }
                }
            }
        }
        rec.cir = cir.clone();
        for (e, rho) in cir {
            if rho > 0 {
                self.residual.inject_cir(&self.graph, e, rho);
            }
        }
    }

    fn inflight_load(&self) -> Vec<u64> {
        let n = self.graph.links.len();
        let mut load = vec![0u64; n];
        for id in self.inflight.keys() {
            let Some(f) = self.flows.get(id) else {
                continue;
            };
            for &e in &f.path.links {
                let i = e.0 as usize;
                if i < n {
                    load[i] = load[i].saturating_add(f.rate_Bps);
                }
            }
        }
        load
    }

    fn recompute_realized(&mut self) {
        if self.inflight.is_empty() {
            return;
        }
        let leftover = self.residual.capacity_leftover(&self.graph);
        let ids: Vec<FlowId> = self.inflight.keys().copied().collect();
        let paths: Vec<Path> = ids
            .iter()
            .filter_map(|id| self.flows.get(id).map(|f| f.path.clone()))
            .collect();
        match water_fill(&paths, &leftover) {
            Ok(rates) => {
                for (id, rate) in ids.iter().zip(rates) {
                    if let Some(f) = self.flows.get_mut(id) {
                        f.rate_Bps = rate;
                    }
                }
            }
            Err(_) => {
                for id in &ids {
                    if let Some(f) = self.flows.get_mut(id) {
                        f.rate_Bps = 0;
                    }
                }
            }
        }
        self.scale_live();
    }

    fn scale_live(&mut self) {
        for _ in 0..8 {
            let load = self.inflight_load();
            let mut changed = false;
            for (i, link) in self.graph.links.iter().enumerate() {
                let cap = if link.failed { 0 } else { link.capacity_Bps };
                let ld = load[i];
                if cap == 0 {
                    for id in self.inflight.keys().copied().collect::<Vec<_>>() {
                        if let Some(f) = self.flows.get_mut(&id) {
                            if f.path.links.iter().any(|e| e.0 as usize == i) && f.rate_Bps != 0 {
                                f.rate_Bps = 0;
                                changed = true;
                            }
                        }
                    }
                    continue;
                }
                if ld > cap {
                    for id in self.inflight.keys().copied().collect::<Vec<_>>() {
                        if let Some(f) = self.flows.get_mut(&id) {
                            if f.path.links.iter().any(|e| e.0 as usize == i) {
                                let new = (f.rate_Bps as u128 * cap as u128 / ld as u128) as u64;
                                if new != f.rate_Bps {
                                    f.rate_Bps = new;
                                    changed = true;
                                }
                            }
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn check_i1(&mut self) -> Result<(), RunError> {
        let load = self.inflight_load();
        for (i, link) in self.graph.links.iter().enumerate() {
            if load[i] > link.capacity_Bps {
                self.invariants_ok = false;
                if self.strict {
                    return Err(RunError::Inv("I1".into()));
                }
            }
        }
        if self.policy == Policy::Joint {
            for (i, link) in self.graph.links.iter().enumerate() {
                let cir = self.residual.cir.get(i).copied().unwrap_or(0);
                let cap95 = Residual::admissible(&self.graph, link.id);
                if cir > cap95 {
                    self.invariants_ok = false;
                    if self.strict {
                        return Err(RunError::Inv("I1".into()));
                    }
                }
            }
        }
        Ok(())
    }

    fn write_admit(
        &mut self,
        spec: &fabric_model::JobSpec,
        free: usize,
        ok: bool,
        reject: Option<RejectCode>,
    ) -> Result<(), RunError> {
        let rec = self.table.by_id.get(&spec.id);
        let admit_seq = rec.and_then(|r| r.admit_seq).map(|s| s.0);
        let t_pred = rec.map(|r| r.t_pred_ps).unwrap_or(0);
        let t_pred_json = ps_json(t_pred);
        let gpu_ids: Vec<u32> = rec
            .and_then(|r| r.binding.as_ref())
            .map(|b| b.map.iter().map(|(_, g)| g.0).collect())
            .unwrap_or_default();
        let map: Vec<Vec<u32>> = rec
            .and_then(|r| r.binding.as_ref())
            .map(|b| b.map.iter().map(|(rk, g)| vec![rk.0, g.0]).collect())
            .unwrap_or_default();
        let b_eff = rec
            .map(|r| {
                r.planned
                    .iter()
                    .map(|f| f.rate_Bps)
                    .filter(|&x| x > 0)
                    .min()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        let would_miss = t_pred > spec.deadline_ps;
        let notes: &[BindingNote] = rec.map(|r| r.notes.as_slice()).unwrap_or(&[]);
        let bindings_evaluated = if notes.is_empty() { 1 } else { notes.len() };
        let per_binding = if notes.is_empty() {
            json!([{
                "kind": "NaiveFirstFit",
                "gpu_ids": gpu_ids,
                "cost": 0,
                "T_pred_ps": t_pred_json,
                "D_j_ps": spec.deadline_ps as i64,
                "code": reject.map(|c| c.as_str()),
                "phase0_links": []
            }])
        } else {
            json!(notes
                .iter()
                .map(|n| {
                    json!({
                        "kind": binding_kind_label(n.kind),
                        "gpu_ids": n.gpu_ids.iter().map(|g| g.0).collect::<Vec<_>>(),
                        "cost": n.cost,
                        "T_pred_ps": ps_json(n.t_pred_ps),
                        "D_j_ps": spec.deadline_ps as i64,
                        "code": n.code.map(|c| c.as_str()),
                        "phase0_links": n.phase0_links.iter().map(|e| e.0).collect::<Vec<_>>(),
                    })
                })
                .collect::<Vec<_>>())
        };
        let chosen_kind = rec
            .and_then(|r| r.binding.as_ref())
            .map(|b| binding_kind_label(b.kind))
            .unwrap_or_else(|| "NaiveFirstFit".into());
        let chosen_idx = rec.and_then(|r| r.chosen_idx).unwrap_or(0);
        let per_link = rec
            .map(|r| {
                r.cir
                    .iter()
                    .map(|(&e, &rho)| {
                        let link = self.graph.link(e);
                        json!({
                            "link_id": e.0,
                            "c_Bps": link.map(|l| l.capacity_Bps).unwrap_or(0),
                            "cir_Bps": self.residual.cir.get(e.0 as usize).copied().unwrap_or(0),
                            "r_avail_Bps": self.residual.r_avail(e),
                            "cost_e": self.residual.cost(&self.graph, e),
                            "rho_job": rho,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let waterfill = rec
            .map(|r| {
                r.planned
                    .iter()
                    .enumerate()
                    .map(|(i, f)| json!({"ord": i, "rate_Bps": f.rate_Bps}))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let naive_gpus = notes
            .iter()
            .find(|n| {
                matches!(
                    n.kind,
                    fabric_types::BindingKind::FirstFitShift { skip_free_gpus: 0 }
                )
            })
            .map(|n| n.gpu_ids.iter().map(|g| g.0).collect::<Vec<_>>())
            .unwrap_or_else(|| gpu_ids.clone());
        let naive_t = notes
            .iter()
            .find(|n| {
                matches!(
                    n.kind,
                    fabric_types::BindingKind::FirstFitShift { skip_free_gpus: 0 }
                )
            })
            .map(|n| n.t_pred_ps)
            .unwrap_or(t_pred);
        let naive_miss = notes
            .iter()
            .find(|n| {
                matches!(
                    n.kind,
                    fabric_types::BindingKind::FirstFitShift { skip_free_gpus: 0 }
                )
            })
            .map(|n| n.code.is_some() || n.t_pred_ps > spec.deadline_ps)
            .unwrap_or(would_miss);
        let v = json!({
            "job_id": spec.id.0,
            "admit_seq": admit_seq,
            "policy": self.policy.as_str(),
            "decision": if ok { "admit" } else { "reject" },
            "reject": reject.map(|c| c.as_str()),
            "free_at_arrive": free,
            "bindings_evaluated": bindings_evaluated,
            "per_binding": per_binding,
            "chosen": if ok {
                json!({"index": chosen_idx, "kind": chosen_kind, "map": map})
            } else {
                serde_json::Value::Null
            },
            "per_link": per_link,
            "waterfill": waterfill,
            "B_eff_Bps": b_eff,
            "T_pred_ps": t_pred_json,
            "D_j_ps": spec.deadline_ps as i64,
            "naive_compare": {
                "gpu_ids": naive_gpus,
                "T_pred_ps": ps_json(naive_t),
                "would_miss_slo": naive_miss
            }
        });
        self.traces.admit_line(&v)?;
        Ok(())
    }

    fn write_job_trace(&mut self, id: JobId, decision: &str, now: i128) {
        let Some(rec) = self.table.by_id.get(&id) else {
            return;
        };
        let t_pred = if rec.t_pred_ps == i128::MAX {
            i64::MAX
        } else {
            ps_i64(rec.t_pred_ps)
        };
        self.traces.job(TraceJob {
            job_id: id.0,
            arrive_ps: ps_i64(rec.spec.arrive.ps),
            exit_ps: ps_i64(now),
            decision: decision.to_string(),
            reject: rec.reject.map(|c| c.as_str().to_string()),
            binding_kind: rec.binding.as_ref().map(|b| b.kind.as_str().to_string()),
            t_pred_ps: t_pred,
            d_j_ps: ps_i64(rec.spec.deadline_ps),
            steps_done: rec.steps_done,
        });
    }

    fn finish(self) -> Result<RunSnapshot, RunError> {
        let mut report = Report::new(
            self.seed,
            self.policy,
            TopoSummary {
                gpus: self.graph.gpus.len() as u32,
                N: self.graph.params.nodes,
                L: self.graph.leaves.len() as u32,
                S: self.graph.spines.len() as u32,
                E_host: self.graph.e_host(),
                E_ls: self.graph.e_ls(),
                B_bisect_gbps: self.graph.b_bisect_gbps(),
            },
        );
        report.mix_hash = self.mix_hash;
        report.topo_hash = self.topo_hash;
        report.horizon_ps = self.horizon_ps;
        report.counts = self.counts.clone();
        report.rejects_by_code = self.rejects_by_code.clone();
        report.invariants_ok = self.invariants_ok;
        report.fails = self.fail_log.clone();

        let mut last_max = 0i128;
        for &d in &self.collective_durs {
            if d > last_max {
                last_max = d;
            }
        }
        let p99 = tail_p99_ps(self.collective_durs.clone());
        let e = self.graph.links.len() as i128;
        let t = self.horizon_ps;
        let mut util_acc = 0i128;
        for (i, link) in self.graph.links.iter().enumerate() {
            if link.capacity_Bps > 0 {
                util_acc = util_acc.saturating_add(
                    self.rate_dt[i].saturating_mul(1_000_000) / (link.capacity_Bps as i128),
                );
            }
        }
        let mean_ppm = if e == 0 || t == 0 {
            0
        } else {
            util_acc / (e * t)
        };

        let mut completions_by_deadline = 0u64;
        let mut slo_miss_jobs = 0u64;
        let mut jobs: Vec<JobRow> = Vec::new();
        for (id, rec) in &self.table.by_id {
            let decision = match rec.state {
                JobState::Rejected => "reject",
                JobState::Killed => "kill",
                JobState::Completed => "admit",
                _ => "admit",
            };
            jobs.push(JobRow {
                job_id: id.0,
                decision: decision.into(),
                steps_done: rec.steps_done,
                t_pred_ps: rec.t_pred_ps,
                reject: rec.reject.map(|c| c.as_str().to_string()),
            });
            if rec.state == JobState::Completed {
                let exit = self.job_exit.get(id).copied().unwrap_or(0);
                let budget = rec.spec.arrive.ps.saturating_add(
                    (rec.spec.step_count as i128)
                        .saturating_mul(rec.spec.compute_ps.saturating_add(rec.spec.deadline_ps)),
                );
                let slo = self.job_slo_ok.get(id).copied().unwrap_or(true);
                if exit <= budget && slo {
                    completions_by_deadline += 1;
                }
                if !slo {
                    slo_miss_jobs += 1;
                }
            }
        }
        report.counts.slo_misses = slo_miss_jobs;
        report.jobs = jobs;
        report.metrics = Metrics {
            hotspot_us: us_of(self.hotspot_ps),
            hotspot_threshold_ppm: 800_000,
            completions_by_deadline,
            tail_collective_us_p99: us_of(p99),
            last_flow_collective_us_max: us_of(last_max),
            slo_miss_us: us_of(self.job_slo_miss_ps),
            disrupted_step_us: us_of(self.disrupted_ps),
            mean_link_util_ppm: mean_ppm,
        };
        self.traces.finish()?;
        Ok(RunSnapshot {
            report,
            epoch: self.graph.epoch,
            event_trace: self.event_trace,
            bytes_epoch: self.bytes_epoch,
            graph: (*self.graph).clone(),
        })
    }
}

fn ps_json(t: i128) -> serde_json::Value {
    if t == i128::MAX {
        serde_json::Value::Null
    } else {
        json!(t as i64)
    }
}

fn binding_kind_label(k: fabric_types::BindingKind) -> String {
    match k {
        fabric_types::BindingKind::NaiveFirstFit => "NaiveFirstFit".into(),
        fabric_types::BindingKind::FirstFitShift { skip_free_gpus } => {
            format!("FirstFitShift{{{skip_free_gpus}}}")
        }
        fabric_types::BindingKind::RailRotate { start_rail } => {
            format!("RailRotate{{{start_rail}}}")
        }
    }
}

/// Serial offset of `phase` within communicator `comm`, using stored rates.
fn phase_offset(planned: &[PlannedFlow], comm: u32, phase: u32, _now: i128) -> i128 {
    let mut acc = 0i128;
    let mut phis: Vec<u32> = planned
        .iter()
        .filter(|f| f.comm == comm && f.phase < phase)
        .map(|f| f.phase)
        .collect();
    phis.sort_unstable();
    phis.dedup();
    for p in phis {
        let group: Vec<&PlannedFlow> = planned
            .iter()
            .filter(|f| f.comm == comm && f.phase == p)
            .collect();
        let b_eff = group.iter().map(|f| f.rate_Bps).min().unwrap_or(0);
        let chunk = group.first().map(|f| f.bytes).unwrap_or(0);
        let d = phase_duration_ps(chunk, b_eff);
        if d == i128::MAX {
            return i128::MAX / 4;
        }
        acc = acc.saturating_add(d);
    }
    acc
}
