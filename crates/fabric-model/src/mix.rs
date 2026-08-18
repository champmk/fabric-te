//! Mix TOML loader. Transcribed from docs/DESIGN.md §9.6.

use std::fmt;
use std::path::{Path, PathBuf};

use fabric_types::{CollectiveKind, JobId, ProcessExit, SimTime};
use serde::Deserialize;

use crate::{isolated_t_ps, s_to_ps};

/// Loaded mix: seed, horizon, jobs in admit order `(arrive_ps, file_index)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mix {
    pub seed: u64,
    pub horizon_ps: i128,
    pub jobs: Vec<JobSpec>,
}

/// Job after load. TOML `id` is `JobId`; never reassigned. §9.4.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobSpec {
    pub id: JobId,
    pub arrive: SimTime,
    pub gpu_count: u32,
    pub dp: u32,
    pub tp: u32,
    pub pp: u32,
    pub collective: CollectiveKind,
    pub payload_bytes: u64,
    pub step_count: u32,
    pub compute_ps: i128,
    pub deadline_ps: i128,
}

#[derive(Debug)]
pub enum MixError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse(String),
    Schema(String),
}

impl MixError {
    pub fn exit(&self) -> ProcessExit {
        match self {
            MixError::Io { .. } => ProcessExit::IoAbort,
            MixError::Parse(_) | MixError::Schema(_) => ProcessExit::BadInput,
        }
    }
}

impl fmt::Display for MixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MixError::Io { path, source } => {
                write!(f, "error[E_IO]: {}: {source}", path.display())
            }
            MixError::Parse(msg) => write!(f, "error[E_PARSE]: {msg}"),
            MixError::Schema(msg) => write!(f, "error[E_SCHEMA]: {msg}"),
        }
    }
}

impl std::error::Error for MixError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MixError::Io { source, .. } => Some(source),
            MixError::Parse(_) | MixError::Schema(_) => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MixFile {
    #[serde(default = "default_seed")]
    seed: u64,
    horizon_s: f64,
    #[serde(default)]
    jobs: Vec<JobFile>,
    #[serde(default)]
    pattern: Vec<PatternFile>,
}

fn default_seed() -> u64 {
    1
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JobFile {
    id: u32,
    arrive_s: f64,
    gpu_count: u32,
    dp: u32,
    tp: u32,
    pp: u32,
    collective: CollectiveToml,
    payload_bytes: u64,
    #[serde(default = "default_step_count")]
    step_count: u32,
    #[serde(default = "default_compute_s")]
    compute_s: f64,
    deadline_s: Option<f64>,
}

fn default_step_count() -> u32 {
    100
}

fn default_compute_s() -> f64 {
    0.010
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PatternFile {
    #[serde(default)]
    #[allow(dead_code)]
    name: Option<String>,
    every_s: f64,
    count: u32,
    start_s: f64,
    start_id: u32,
    gpu_count: u32,
    dp: u32,
    tp: u32,
    pp: u32,
    collective: CollectiveToml,
    payload_bytes: u64,
    #[serde(default = "default_step_count")]
    step_count: u32,
    #[serde(default = "default_compute_s")]
    compute_s: f64,
    deadline_s: Option<f64>,
}

#[derive(Copy, Clone, Deserialize)]
enum CollectiveToml {
    #[serde(rename = "ring_allreduce")]
    RingAllReduce,
    #[serde(rename = "pairwise_alltoall")]
    PairwiseAllToAll,
}

impl From<CollectiveToml> for CollectiveKind {
    fn from(c: CollectiveToml) -> Self {
        match c {
            CollectiveToml::RingAllReduce => CollectiveKind::RingAllReduce,
            CollectiveToml::PairwiseAllToAll => CollectiveKind::PairwiseAllToAll,
        }
    }
}

struct RawJob {
    file_index: usize,
    id: u32,
    arrive_s: f64,
    gpu_count: u32,
    dp: u32,
    tp: u32,
    pp: u32,
    collective: CollectiveKind,
    payload_bytes: u64,
    step_count: u32,
    compute_s: f64,
    deadline_s: Option<f64>,
}

/// Flatten `[[jobs]]` then expanded `[[pattern]]`. Sort `(arrive_ps, file_index)`.
pub fn load_mix(path: impl AsRef<Path>) -> Result<Mix, MixError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|source| MixError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let file: MixFile = toml::from_str(&text).map_err(|e| MixError::Parse(e.to_string()))?;

    let mut raw: Vec<RawJob> = Vec::new();
    for job in file.jobs {
        raw.push(RawJob {
            file_index: 0,
            id: job.id,
            arrive_s: job.arrive_s,
            gpu_count: job.gpu_count,
            dp: job.dp,
            tp: job.tp,
            pp: job.pp,
            collective: job.collective.into(),
            payload_bytes: job.payload_bytes,
            step_count: job.step_count,
            compute_s: job.compute_s,
            deadline_s: job.deadline_s,
        });
    }
    const MAX_JOBS: usize = 1_000_000;
    for pat in file.pattern {
        if raw.len().saturating_add(pat.count as usize) > MAX_JOBS {
            return Err(MixError::Schema("pattern expands past 1000000 jobs".into()));
        }
        for i in 0..pat.count {
            let id = pat
                .start_id
                .checked_add(i)
                .ok_or_else(|| MixError::Schema("pattern start_id + i overflows u32".into()))?;
            raw.push(RawJob {
                file_index: 0,
                id,
                arrive_s: pat.start_s + f64::from(i) * pat.every_s,
                gpu_count: pat.gpu_count,
                dp: pat.dp,
                tp: pat.tp,
                pp: pat.pp,
                collective: pat.collective.into(),
                payload_bytes: pat.payload_bytes,
                step_count: pat.step_count,
                compute_s: pat.compute_s,
                deadline_s: pat.deadline_s,
            });
        }
    }
    for (idx, row) in raw.iter_mut().enumerate() {
        row.file_index = idx;
    }

    let mut keyed: Vec<(i128, usize, RawJob)> = Vec::with_capacity(raw.len());
    for row in raw {
        let arrive_ps = seconds_to_ps(row.arrive_s)?;
        let file_index = row.file_index;
        keyed.push((arrive_ps, file_index, row));
    }
    keyed.sort_by_key(|(arrive_ps, file_index, _)| (*arrive_ps, *file_index));

    let mut jobs = Vec::with_capacity(keyed.len());
    for (seq, (arrive_ps, _, row)) in keyed.into_iter().enumerate() {
        check_shape(&row)?;
        let compute_ps = seconds_to_ps(row.compute_s)?;
        let deadline_ps = match row.deadline_s {
            Some(d) => seconds_to_ps(d)?,
            None => 2 * isolated_t_ps(row.collective, row.dp, row.payload_bytes),
        };
        jobs.push(JobSpec {
            id: JobId(row.id),
            arrive: SimTime {
                ps: arrive_ps,
                seq: seq as u64,
            },
            gpu_count: row.gpu_count,
            dp: row.dp,
            tp: row.tp,
            pp: row.pp,
            collective: row.collective,
            payload_bytes: row.payload_bytes,
            step_count: row.step_count,
            compute_ps,
            deadline_ps,
        });
    }

    Ok(Mix {
        seed: file.seed,
        horizon_ps: seconds_to_ps(file.horizon_s)?,
        jobs,
    })
}

fn seconds_to_ps(x: f64) -> Result<i128, MixError> {
    if !x.is_finite() {
        return Err(MixError::Parse("non-finite seconds".into()));
    }
    Ok(s_to_ps(x))
}

fn check_shape(row: &RawJob) -> Result<(), MixError> {
    let product = u64::from(row.dp) * u64::from(row.tp) * u64::from(row.pp);
    if product != u64::from(row.gpu_count) {
        return Err(MixError::Schema("dp*tp*pp != gpu_count".into()));
    }
    Ok(())
}

/// Isolated T at 47.5e9 B/s, p=dp, full-fabric. T>deadline or gpu_count>g_tot → MixDoesNotFit.
pub fn check_isolated(mix: &Mix, g_tot: u32) -> Result<(), ProcessExit> {
    for job in &mix.jobs {
        if job.gpu_count > g_tot {
            return Err(ProcessExit::MixDoesNotFit);
        }
        let t = isolated_t_ps(job.collective, job.dp, job.payload_bytes);
        if t > job.deadline_ps {
            return Err(ProcessExit::MixDoesNotFit);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn mix_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/mix")
            .join(name)
    }

    fn write_temp(tag: &str, body: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("fabric-te-pr3-{}-{}.toml", tag, std::process::id()));
        let mut f = std::fs::File::create(&p).expect("temp mix");
        f.write_all(body.as_bytes()).expect("write temp mix");
        p
    }

    #[test]
    fn empty_mix_loads() {
        let mix = load_mix(mix_path("empty.toml")).expect("empty.toml");
        assert_eq!(mix.horizon_ps, 1_000_000_000_000);
        assert!(mix.jobs.is_empty());
        assert_eq!(check_isolated(&mix, 256), Ok(()));
    }

    #[test]
    fn unknown_key_is_exit_2() {
        let p = write_temp("jitter", "horizon_s = 1\njitter = 0.1\n");
        let err = load_mix(&p).expect_err("unknown key");
        assert_eq!(err.exit(), ProcessExit::BadInput);
        assert_eq!(err.exit() as i32, 2);
    }

    #[test]
    fn shape_mismatch_is_exit_2() {
        let p = write_temp(
            "shape",
            r#"
horizon_s = 1
[[jobs]]
id = 1
arrive_s = 0.0
gpu_count = 16
dp = 2
tp = 2
pp = 1
collective = "ring_allreduce"
payload_bytes = 67108864
"#,
        );
        let err = load_mix(&p).expect_err("shape");
        assert_eq!(err.exit(), ProcessExit::BadInput);
        assert_eq!(err.exit() as i32, 2);
    }

    #[test]
    fn pattern_keeps_ids_and_sorts() {
        let p = write_temp(
            "pattern",
            r#"
horizon_s = 10
[[jobs]]
id = 1
arrive_s = 2.0
gpu_count = 2
dp = 2
tp = 1
pp = 1
collective = "ring_allreduce"
payload_bytes = 67108864
deadline_s = 0.005

[[pattern]]
name = "steady-dp"
every_s = 1.0
count = 2
start_s = 0.5
start_id = 100
gpu_count = 2
dp = 2
tp = 1
pp = 1
collective = "ring_allreduce"
payload_bytes = 67108864
deadline_s = 0.005
"#,
        );
        let mix = load_mix(&p).expect("pattern mix");
        let ids: Vec<u32> = mix.jobs.iter().map(|j| j.id.0).collect();
        // file order: id=1 @2s, then 100 @0.5s, 101 @1.5s. Sort by arrive, then file_index.
        assert_eq!(ids, vec![100, 101, 1]);
        assert_eq!(mix.jobs[0].arrive.ps, s_to_ps(0.5));
        assert_eq!(mix.jobs[0].arrive.seq, 0);
        assert_eq!(mix.jobs[1].arrive.ps, s_to_ps(1.5));
        assert_eq!(mix.jobs[2].arrive.ps, s_to_ps(2.0));
    }

    #[test]
    fn omitted_deadline_is_twice_isolated() {
        let p = write_temp(
            "nodeadline",
            r#"
horizon_s = 1
[[jobs]]
id = 7
arrive_s = 0.0
gpu_count = 8
dp = 8
tp = 1
pp = 1
collective = "ring_allreduce"
payload_bytes = 67108864
"#,
        );
        let mix = load_mix(&p).expect("no deadline");
        let t = isolated_t_ps(CollectiveKind::RingAllReduce, 8, 67_108_864);
        assert_eq!(mix.jobs[0].deadline_ps, 2 * t);
        assert_eq!(check_isolated(&mix, 256), Ok(()));
    }

    #[test]
    fn isolated_slo_exit_4() {
        let p = write_temp(
            "slo",
            r#"
horizon_s = 1
[[jobs]]
id = 1
arrive_s = 0.0
gpu_count = 8
dp = 8
tp = 1
pp = 1
collective = "ring_allreduce"
payload_bytes = 67108864
deadline_s = 0.000001
"#,
        );
        let mix = load_mix(&p).expect("load");
        assert_eq!(check_isolated(&mix, 256), Err(ProcessExit::MixDoesNotFit));
        assert_eq!(ProcessExit::MixDoesNotFit as i32, 4);
    }

    #[test]
    fn gpu_count_over_gtot_exit_4() {
        let p = write_temp(
            "big",
            r#"
horizon_s = 1
[[jobs]]
id = 1
arrive_s = 0.0
gpu_count = 8
dp = 8
tp = 1
pp = 1
collective = "ring_allreduce"
payload_bytes = 67108864
deadline_s = 1.0
"#,
        );
        let mix = load_mix(&p).expect("load");
        assert_eq!(check_isolated(&mix, 4), Err(ProcessExit::MixDoesNotFit));
    }

    #[test]
    fn joint_reject_slo_miss() {
        // Isolated T at 47.5 GB/s > D_j → exit 4 at load. No bypass.
        let p = write_temp(
            "joint-slo",
            r#"
horizon_s = 1
[[jobs]]
id = 1
arrive_s = 0.0
gpu_count = 8
dp = 8
tp = 1
pp = 1
collective = "ring_allreduce"
payload_bytes = 67108864
deadline_s = 0.001
"#,
        );
        let mix = load_mix(&p).expect("load");
        // isolated ring p=8 at 47.5e9 ≈ 2486 µs > 1000 µs
        assert_eq!(check_isolated(&mix, 512), Err(ProcessExit::MixDoesNotFit));
    }
}
