//! Integer-µs `report.json`. Transcribed from docs/DESIGN.md §9.8, §16.2, §17.

#![forbid(unsafe_code)]
#![allow(non_snake_case)]

mod html;

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use fabric_types::{ps_to_us, Policy, ProcessExit, RejectCode};
use serde::Serialize;

pub use html::write_html;

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub spec_version: String,
    pub seed: u64,
    pub policy: String,
    pub topo: TopoSummary,
    pub mix_hash: String,
    pub topo_hash: String,
    pub horizon_ps: i128,
    pub counts: Counts,
    pub rejects_by_code: BTreeMap<String, u64>,
    pub metrics: Metrics,
    pub jobs: Vec<JobRow>,
    pub fails: Vec<serde_json::Value>,
    pub invariants_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanReport>,
}

/// `report.json` `"plan"` object. §15.2
#[derive(Clone, Debug, Serialize)]
pub struct PlanReport {
    pub deltas: Vec<String>,
    pub nodes_removed: Vec<u32>,
    pub gpus_removed: u32,
    pub S_before: u32,
    pub S_after: u32,
    pub jobs_admitted: Vec<u32>,
    pub jobs_rejected: Vec<PlanReject>,
    pub new_hotspots: Vec<u32>,
    pub restore: PlanRestore,
    pub vs_baseline: PlanBaseline,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlanReject {
    pub id: u32,
    pub code: String,
    pub T_pred_ps: i128,
    pub D_j_ps: i128,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlanRestore {
    pub extra_spines: Option<u32>,
    pub rows_needed: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlanBaseline {
    pub admits: u64,
    pub rejects: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct TopoSummary {
    pub gpus: u32,
    pub N: u32,
    pub L: u32,
    pub S: u32,
    pub E_host: u32,
    pub E_ls: u32,
    pub B_bisect_gbps: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Counts {
    pub arrivals: u64,
    pub admits: u64,
    pub rejects: u64,
    pub kills: u64,
    pub completes: u64,
    pub slo_misses: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Metrics {
    pub hotspot_us: i128,
    pub hotspot_threshold_ppm: u64,
    pub completions_by_deadline: u64,
    pub tail_collective_us_p99: i128,
    pub last_flow_collective_us_max: i128,
    pub slo_miss_us: i128,
    pub disrupted_step_us: i128,
    pub mean_link_util_ppm: i128,
}

#[derive(Clone, Debug, Serialize)]
pub struct JobRow {
    pub job_id: u32,
    pub decision: String,
    pub steps_done: u32,
    pub t_pred_ps: i128,
    pub reject: Option<String>,
}

impl Report {
    pub fn empty_rejects() -> BTreeMap<String, u64> {
        let mut m = BTreeMap::new();
        for c in RejectCode::ALL {
            m.insert(c.as_str().to_string(), 0);
        }
        m
    }

    pub fn new(seed: u64, policy: Policy, topo: TopoSummary) -> Self {
        Self {
            spec_version: "0.1".into(),
            seed,
            policy: policy.as_str().to_string(),
            topo,
            mix_hash: String::new(),
            topo_hash: String::new(),
            horizon_ps: 0,
            counts: Counts::default(),
            rejects_by_code: Self::empty_rejects(),
            metrics: Metrics {
                hotspot_threshold_ppm: 800_000,
                ..Metrics::default()
            },
            jobs: Vec::new(),
            fails: Vec::new(),
            invariants_ok: true,
            plan: None,
        }
    }

    pub fn write_json(&self, path: &Path) -> Result<(), io::Error> {
        let s = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(path, s)
    }

    /// counts keys then every rejects_by_code key, one row, integer cells. §16.2
    pub fn stdout_summary(&self) -> String {
        let mut keys = vec![
            "arrivals",
            "admits",
            "rejects",
            "kills",
            "completes",
            "slo_misses",
        ];
        for c in RejectCode::ALL {
            keys.push(c.as_str());
        }
        let mut vals: Vec<String> = vec![
            self.counts.arrivals.to_string(),
            self.counts.admits.to_string(),
            self.counts.rejects.to_string(),
            self.counts.kills.to_string(),
            self.counts.completes.to_string(),
            self.counts.slo_misses.to_string(),
        ];
        for c in RejectCode::ALL {
            let n = self.rejects_by_code.get(c.as_str()).copied().unwrap_or(0);
            vals.push(n.to_string());
        }
        format!("{}\n{}", keys.join(" "), vals.join(" "))
    }

    pub fn print_stdout(&self) -> Result<(), ProcessExit> {
        let s = self.stdout_summary();
        let mut out = io::stdout();
        if writeln!(out, "{s}").is_err() {
            return Err(ProcessExit::IoAbort);
        }
        Ok(())
    }
}

pub fn us_of(ps: i128) -> i128 {
    ps_to_us(ps)
}

/// p99 of collective durations in ps; n<100 → max. §17
pub fn tail_p99_ps(mut xs: Vec<i128>) -> i128 {
    if xs.is_empty() {
        return 0;
    }
    xs.sort_unstable();
    if xs.len() < 100 {
        return *xs.last().unwrap();
    }
    let idx = (xs.len() * 99).div_ceil(100).saturating_sub(1);
    xs[idx.min(xs.len() - 1)]
}
