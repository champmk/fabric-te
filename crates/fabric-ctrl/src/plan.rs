//! Planner: same admit engine on a delta-modified Graph. §15.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use fabric_model::{check_isolated, Mix};
use fabric_report::{PlanBaseline, PlanReject, PlanReport, PlanRestore, Report};
use fabric_topo::Graph;
use fabric_types::{GpuAvail, Policy, ProcessExit, UnavailReason};

use crate::epoch::FailSpec;
use crate::kernel::{run_sim_snapshot, RunConfig, RunError, RunSnapshot};
use crate::table::Occupancy;

static SCRATCH_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Delta {
    DelayRow(u32),
    Spines(u32),
    SpinesPercent(u32),
    Oversub(u32),
}

pub struct PlanConfig {
    pub graph: Graph,
    pub mix: Mix,
    pub policy: Policy,
    pub seed: u64,
    pub out: PathBuf,
    pub strict: bool,
    pub mix_hash: String,
    pub topo_hash: String,
    pub fails: Vec<FailSpec>,
    pub deltas: Vec<Delta>,
    pub delta_specs: Vec<String>,
}

pub struct PlanOutcome {
    pub report: Report,
    pub snapshot: RunSnapshot,
    pub mix_does_not_fit: bool,
}

#[derive(Debug)]
pub enum PlanError {
    Delta(String),
    Run(RunError),
}

impl PlanError {
    pub fn exit(&self) -> ProcessExit {
        match self {
            PlanError::Delta(_) => ProcessExit::BadInput,
            PlanError::Run(e) => e.exit(),
        }
    }

    pub fn e_code(&self) -> &'static str {
        match self {
            PlanError::Delta(_) => "E_FAILSPEC",
            PlanError::Run(e) => e.e_code(),
        }
    }
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::Delta(m) => write!(f, "error[E_FAILSPEC]: {m}"),
            PlanError::Run(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PlanError {}

/// `--delta` grammar §15.1. Unknown → E_FAILSPEC.
pub fn parse_delta(s: &str) -> Result<Delta, String> {
    let (key, val) = s
        .split_once('=')
        .ok_or_else(|| format!("expected key=value, got {s}"))?;
    match key {
        "delay-row" => {
            let b = val.as_bytes();
            if b.len() != 1 || !b[0].is_ascii_alphabetic() {
                return Err(format!("bad delay-row {val}"));
            }
            let c = b[0].to_ascii_uppercase();
            Ok(Delta::DelayRow((c - b'A') as u32))
        }
        "spines" => {
            if let Some(p) = val
                .strip_prefix('-')
                .and_then(|rest| rest.strip_suffix('%'))
            {
                let p: u32 = p.parse().map_err(|_| format!("bad spines percent {val}"))?;
                if p > 100 {
                    return Err(format!("bad spines percent {val}"));
                }
                Ok(Delta::SpinesPercent(p))
            } else {
                let n: u32 = val.parse().map_err(|_| format!("bad spines {val}"))?;
                Ok(Delta::Spines(n))
            }
        }
        "oversub" => {
            let k: u32 = val.parse().map_err(|_| format!("bad oversub {val}"))?;
            if !matches!(k, 1 | 2 | 4 | 8 | 16 | 32) {
                return Err(format!(
                    "oversub must be one of [1, 2, 4, 8, 16, 32], got {k}"
                ));
            }
            Ok(Delta::Oversub(k))
        }
        _ => Err(format!("unknown delta {s}")),
    }
}

/// Apply deltas in CLI order. §15.1
pub fn apply_deltas(graph: &mut Graph, deltas: &[Delta]) -> Result<(), String> {
    for d in deltas {
        match *d {
            Delta::DelayRow(i) => graph.mark_row_absent(i),
            Delta::Spines(n) => graph.rebuild_spines(n),
            Delta::SpinesPercent(p) => {
                let s = graph.spines.len() as u32;
                let s_new = (s as u64)
                    .saturating_mul(100u64.saturating_sub(p as u64))
                    .div_ceil(100) as u32;
                graph.rebuild_spines(s_new);
            }
            Delta::Oversub(k) => {
                graph.rebuild_oversub(k).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

pub fn run_plan(cfg: PlanConfig) -> Result<PlanOutcome, PlanError> {
    let g_tot = cfg.graph.gpus.len() as u32;
    let mix_does_not_fit = check_isolated(&cfg.mix, g_tot).is_err();

    let s_before = cfg.graph.spines.len() as u32;
    let mut delta_graph = cfg.graph.clone();
    apply_deltas(&mut delta_graph, &cfg.deltas).map_err(PlanError::Delta)?;
    let s_after = delta_graph.spines.len() as u32;
    let nodes_removed: Vec<u32> = delta_graph
        .nodes
        .iter()
        .filter(|n| !n.present)
        .map(|n| n.id.0)
        .collect();
    let gpus_removed = delta_graph
        .gpus
        .iter()
        .filter(|g| matches!(g.avail, GpuAvail::Unavailable(UnavailReason::AbsentRow)))
        .count() as u32;

    let base_dir = scratch_dir("base");
    let base_snap = run_once(&cfg, cfg.graph.clone(), base_dir.clone())?;
    let _ = std::fs::remove_dir_all(&base_dir);

    let delta_snap = run_once(&cfg, delta_graph.clone(), cfg.out.clone())?;

    let extra_spines =
        restore_extra_spines(&cfg, &delta_graph, s_after, s_before, &delta_snap.report)?;
    let rows_needed = restore_rows_needed(&cfg, &delta_graph, &delta_snap.report)?;

    let base_hot: BTreeSet<u32> = base_snap.hot_links.iter().map(|l| l.0).collect();
    let new_hotspots: Vec<u32> = delta_snap
        .hot_links
        .iter()
        .map(|l| l.0)
        .filter(|id| !base_hot.contains(id))
        .collect();

    let jobs_admitted: Vec<u32> = delta_snap
        .report
        .jobs
        .iter()
        .filter(|j| j.decision == "admit")
        .map(|j| j.job_id)
        .collect();
    let dj: BTreeMap<u32, i128> = cfg
        .mix
        .jobs
        .iter()
        .map(|j| (j.id.0, j.deadline_ps))
        .collect();
    let jobs_rejected: Vec<PlanReject> = delta_snap
        .report
        .jobs
        .iter()
        .filter(|j| j.decision == "reject")
        .map(|j| PlanReject {
            id: j.job_id,
            code: j.reject.clone().unwrap_or_default(),
            T_pred_ps: j.t_pred_ps,
            D_j_ps: dj.get(&j.job_id).copied().unwrap_or(0),
        })
        .collect();

    let mut report = delta_snap.report.clone();
    report.plan = Some(PlanReport {
        deltas: cfg.delta_specs.clone(),
        nodes_removed,
        gpus_removed,
        S_before: s_before,
        S_after: s_after,
        jobs_admitted,
        jobs_rejected,
        new_hotspots,
        restore: PlanRestore {
            extra_spines,
            rows_needed,
        },
        vs_baseline: PlanBaseline {
            admits: base_snap.report.counts.admits,
            rejects: base_snap.report.counts.rejects,
        },
    });

    Ok(PlanOutcome {
        report,
        snapshot: delta_snap,
        mix_does_not_fit,
    })
}

fn run_once(cfg: &PlanConfig, graph: Graph, out: PathBuf) -> Result<RunSnapshot, PlanError> {
    run_sim_snapshot(RunConfig {
        graph,
        mix: cfg.mix.clone(),
        policy: cfg.policy,
        seed: cfg.seed,
        out,
        strict: cfg.strict,
        mix_hash: cfg.mix_hash.clone(),
        topo_hash: cfg.topo_hash.clone(),
        fails: cfg.fails.clone(),
        occupancy: Occupancy::new(),
        residual: None,
    })
    .map_err(PlanError::Run)
}

/// Scan S'..S. `0` if the delta graph already fully admits; `null` if impossible. §15.2
fn restore_extra_spines(
    cfg: &PlanConfig,
    delta_graph: &Graph,
    s_after: u32,
    s_before: u32,
    delta_report: &Report,
) -> Result<Option<u32>, PlanError> {
    if fully_admits(delta_report, &cfg.mix) {
        return Ok(Some(0));
    }
    if s_after >= s_before {
        return Ok(None);
    }
    for s_try in (s_after + 1)..=s_before {
        let mut g = delta_graph.clone();
        g.rebuild_spines(s_try);
        let dir = scratch_dir("spines");
        let snap = run_once(cfg, g, dir.clone())?;
        let _ = std::fs::remove_dir_all(&dir);
        if fully_admits(&snap.report, &cfg.mix) {
            return Ok(Some(s_try - s_after));
        }
    }
    Ok(None)
}

/// Delayed rows whose restore admits every mix job. §15.2
fn restore_rows_needed(
    cfg: &PlanConfig,
    delta_graph: &Graph,
    delta_report: &Report,
) -> Result<Vec<String>, PlanError> {
    let mut delayed: Vec<u32> = Vec::new();
    for d in &cfg.deltas {
        if let Delta::DelayRow(i) = *d {
            if !delayed.contains(&i) {
                delayed.push(i);
            }
        }
    }
    if delayed.is_empty() {
        return Ok(Vec::new());
    }
    if fully_admits(delta_report, &cfg.mix) {
        return Ok(delayed.iter().copied().map(row_letter).collect());
    }
    let mut g = delta_graph.clone();
    for &row in &delayed {
        g.restore_row(row);
    }
    let dir = scratch_dir("rows");
    let snap = run_once(cfg, g, dir.clone())?;
    let _ = std::fs::remove_dir_all(&dir);
    if fully_admits(&snap.report, &cfg.mix) {
        Ok(delayed.iter().copied().map(row_letter).collect())
    } else {
        Ok(Vec::new())
    }
}

/// Every mix job has `decision=admit`. Not “every job the baseline admitted”. §15.2
fn fully_admits(report: &Report, mix: &Mix) -> bool {
    let admitted: BTreeSet<u32> = report
        .jobs
        .iter()
        .filter(|j| j.decision == "admit")
        .map(|j| j.job_id)
        .collect();
    mix.jobs.iter().all(|j| admitted.contains(&j.id.0))
}

fn row_letter(i: u32) -> String {
    if i < 26 {
        ((b'A' + i as u8) as char).to_string()
    } else {
        format!("R{i}")
    }
}

fn scratch_dir(tag: &str) -> PathBuf {
    let n = SCRATCH_SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "fabric-te-plan-{}-{}-{}",
        std::process::id(),
        n,
        tag
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::run_sim;
    use fabric_model::JobSpec;
    use fabric_types::{CollectiveKind, JobId, SimTime};
    use std::collections::BTreeSet;

    fn tiny(id: u32) -> JobSpec {
        JobSpec {
            id: JobId(id),
            arrive: SimTime { ps: 0, seq: 0 },
            gpu_count: 8,
            dp: 8,
            tp: 1,
            pp: 1,
            collective: CollectiveKind::RingAllReduce,
            payload_bytes: 64,
            step_count: 2,
            compute_ps: 1_000_000_000,
            deadline_ps: 10_000_000_000,
        }
    }

    fn mix(jobs: Vec<JobSpec>) -> Mix {
        Mix {
            seed: 1,
            horizon_ps: 20_000_000_000,
            jobs,
        }
    }

    fn cfg(
        graph: Graph,
        mix: Mix,
        out: PathBuf,
        deltas: Vec<Delta>,
        specs: Vec<String>,
    ) -> PlanConfig {
        PlanConfig {
            graph,
            mix,
            policy: Policy::Joint,
            seed: 1,
            out,
            strict: false,
            mix_hash: "t".into(),
            topo_hash: "t".into(),
            fails: Vec::new(),
            deltas,
            delta_specs: specs,
        }
    }

    #[test]
    fn planner_same_engine() {
        let graph = Graph::generate(256, 8, 1).expect("n32");
        let mix = mix(vec![tiny(1), tiny(2)]);
        let run_out = scratch_dir("same-run");
        let plan_out = scratch_dir("same-plan");
        let run_rep = run_sim(RunConfig {
            graph: graph.clone(),
            mix: mix.clone(),
            policy: Policy::Joint,
            seed: 1,
            out: run_out.clone(),
            strict: false,
            mix_hash: "t".into(),
            topo_hash: "t".into(),
            fails: Vec::new(),
            occupancy: Occupancy::new(),
            residual: None,
        })
        .expect("run");
        let plan = run_plan(cfg(graph, mix, plan_out.clone(), vec![], vec![])).expect("plan");
        let admit_run: BTreeSet<u32> = run_rep
            .jobs
            .iter()
            .filter(|j| j.decision == "admit")
            .map(|j| j.job_id)
            .collect();
        let admit_plan: BTreeSet<u32> = plan
            .report
            .jobs
            .iter()
            .filter(|j| j.decision == "admit")
            .map(|j| j.job_id)
            .collect();
        let _ = std::fs::remove_dir_all(&run_out);
        let _ = std::fs::remove_dir_all(&plan_out);
        assert_eq!(admit_run, admit_plan);
        let p = plan.report.plan.expect("plan fields");
        assert_eq!(p.vs_baseline.admits, run_rep.counts.admits);
        assert_eq!(p.vs_baseline.rejects, run_rep.counts.rejects);
    }

    #[test]
    fn planner_delay_row_b() {
        let graph = Graph::generate(256, 8, 1).expect("n32");
        let mix = mix(vec![tiny(1), tiny(2), tiny(3)]);
        let out = scratch_dir("row-b");
        let plan = run_plan(cfg(
            graph,
            mix,
            out.clone(),
            vec![Delta::DelayRow(1)],
            vec!["delay-row=B".into()],
        ))
        .expect("plan");
        let g = &plan.snapshot.graph;
        for n in 16..32 {
            assert!(!g.nodes[n as usize].present, "node {n} still present");
        }
        for gpu in &g.gpus {
            if (16..32).contains(&gpu.node.0) {
                assert_eq!(
                    gpu.avail,
                    GpuAvail::Unavailable(UnavailReason::AbsentRow),
                    "gpu {}",
                    gpu.id.0
                );
            }
        }
        for gid in &plan.snapshot.bound_gpus {
            let node = g.gpu(*gid).map(|x| x.node.0).unwrap_or(u32::MAX);
            assert!(
                !(16..32).contains(&node),
                "bound GpuId {} on node {node}",
                gid.0
            );
        }
        let p = plan.report.plan.expect("plan fields");
        assert_eq!(p.gpus_removed, 128);
        assert_eq!(p.nodes_removed, (16..32).collect::<Vec<_>>());
        assert_eq!(p.deltas, vec!["delay-row=B"]);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn parse_delta_grammar() {
        assert_eq!(parse_delta("delay-row=B").unwrap(), Delta::DelayRow(1));
        assert_eq!(parse_delta("spines=3").unwrap(), Delta::Spines(3));
        assert_eq!(
            parse_delta("spines=-25%").unwrap(),
            Delta::SpinesPercent(25)
        );
        assert_eq!(parse_delta("oversub=2").unwrap(), Delta::Oversub(2));
        assert!(parse_delta("nope=1").is_err());
        assert!(parse_delta("oversub=3").is_err());
    }
}
