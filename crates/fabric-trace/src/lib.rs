//! Parquet traces and admit.jsonl. Transcribed from docs/DESIGN.md §9.7.

#![forbid(unsafe_code)]
#![allow(non_snake_case)]

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, Int64Array, ListBuilder, StringArray, UInt32Array, UInt32Builder, UInt64Array,
    UInt8Array,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use serde_json::Value;

const LINK_ROW_CAP: usize = 50_000;

/// I6: rollup of events / admit.jsonl / jobs.parquet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceRollup {
    pub arrivals: u64,
    pub admits: u64,
    pub rejects: u64,
    pub kills: u64,
    pub completes: u64,
}

#[derive(Debug)]
pub struct TraceError(pub String);

impl std::fmt::Display for TraceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error[E_IO]: {}", self.0)
    }
}

impl std::error::Error for TraceError {}

impl From<std::io::Error> for TraceError {
    fn from(e: std::io::Error) -> Self {
        TraceError(e.to_string())
    }
}

impl From<arrow::error::ArrowError> for TraceError {
    fn from(e: arrow::error::ArrowError) -> Self {
        TraceError(e.to_string())
    }
}

impl From<parquet::errors::ParquetError> for TraceError {
    fn from(e: parquet::errors::ParquetError) -> Self {
        TraceError(e.to_string())
    }
}

pub struct EventRow {
    pub t_ps: i64,
    pub seq: u64,
    pub kind: String,
    pub epoch: u32,
    pub job_id: Option<u32>,
    pub flow_id: Option<u64>,
    pub link_id: Option<u32>,
    pub spine_id: Option<u32>,
    pub leaf_id: Option<u32>,
    pub rail_id: Option<u8>,
    pub reject: Option<String>,
    pub bytes: Option<u64>,
}

pub struct FlowRow {
    pub flow_id: u64,
    pub job_id: u32,
    pub phase: u32,
    pub src_gpu: u32,
    pub dst_gpu: u32,
    pub path_links: Vec<u32>,
    pub rate_Bps: u64,
    pub bytes: u64,
    pub t_arrive_ps: i64,
    pub t_depart_ps: i64,
}

pub struct LinkRow {
    pub link_id: u32,
    pub t_ps: i64,
    pub c_Bps: u64,
    pub cir_Bps: u64,
    pub r_avail_Bps: u64,
    pub q_bytes: u64,
    pub failed: bool,
}

pub struct JobRow {
    pub job_id: u32,
    pub arrive_ps: i64,
    pub exit_ps: i64,
    pub decision: String,
    pub reject: Option<String>,
    pub binding_kind: Option<String>,
    pub t_pred_ps: i64,
    pub d_j_ps: i64,
    pub steps_done: u32,
}

/// Append-only Parquet + admit.jsonl under `--out`.
pub struct TraceSink {
    dir: PathBuf,
    events: Vec<EventRow>,
    flows: Vec<FlowRow>,
    links: Vec<LinkRow>,
    jobs: Vec<JobRow>,
    admit: BufWriter<File>,
    link_written: Vec<u32>,
    link_stride: Vec<u32>,
    link_tick: Vec<u32>,
    admit_ok: u64,
    admit_reject: u64,
}

impl TraceSink {
    pub fn create(dir: &Path, seed: u64) -> Result<Self, TraceError> {
        fs::create_dir_all(dir)?;
        let admit = BufWriter::new(File::create(dir.join("admit.jsonl"))?);
        let meta = format!("seed = {seed}\n");
        fs::write(dir.join("meta.toml"), meta)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            events: Vec::new(),
            flows: Vec::new(),
            links: Vec::new(),
            jobs: Vec::new(),
            admit,
            link_written: Vec::new(),
            link_stride: Vec::new(),
            link_tick: Vec::new(),
            admit_ok: 0,
            admit_reject: 0,
        })
    }

    /// I6: in-memory event/admit/job counts.
    pub fn rollup(&self) -> TraceRollup {
        let mut r = TraceRollup::default();
        for e in &self.events {
            if e.kind == "JobArrive" {
                r.arrivals = r.arrivals.saturating_add(1);
            }
        }
        r.admits = self.admit_ok;
        r.rejects = self.admit_reject;
        for j in &self.jobs {
            match j.decision.as_str() {
                "kill" => r.kills = r.kills.saturating_add(1),
                "admit" => r.completes = r.completes.saturating_add(1),
                _ => {}
            }
        }
        r
    }

    pub fn event(&mut self, row: EventRow) {
        self.events.push(row);
    }

    pub fn flow(&mut self, row: FlowRow) {
        self.flows.push(row);
    }

    pub fn job(&mut self, row: JobRow) {
        self.jobs.push(row);
    }

    /// Cap 50_000 rows/link then stride. §9.7
    pub fn link(&mut self, row: LinkRow) {
        let id = row.link_id as usize;
        if id >= self.link_written.len() {
            self.link_written.resize(id + 1, 0);
            self.link_stride.resize(id + 1, 1);
            self.link_tick.resize(id + 1, 0);
        }
        self.link_tick[id] = self.link_tick[id].saturating_add(1);
        if self.link_tick[id] % self.link_stride[id] != 0 {
            return;
        }
        if self.link_written[id] as usize >= LINK_ROW_CAP {
            self.link_stride[id] = self.link_stride[id].saturating_mul(2).max(2);
            self.link_tick[id] = 0;
            return;
        }
        self.link_written[id] = self.link_written[id].saturating_add(1);
        self.links.push(row);
    }

    pub fn admit_line(&mut self, v: &Value) -> Result<(), TraceError> {
        serde_json::to_writer(&mut self.admit, v).map_err(|e| TraceError(e.to_string()))?;
        self.admit.write_all(b"\n")?;
        match v.get("decision").and_then(|d| d.as_str()) {
            Some("admit") => self.admit_ok = self.admit_ok.saturating_add(1),
            Some("reject") => self.admit_reject = self.admit_reject.saturating_add(1),
            _ => {}
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<(), TraceError> {
        self.admit.flush()?;
        write_events(&self.dir, &self.events)?;
        write_flows(&self.dir, &self.flows)?;
        write_links(&self.dir, &self.links)?;
        write_jobs(&self.dir, &self.jobs)?;
        Ok(())
    }
}

fn props() -> WriterProperties {
    WriterProperties::builder()
        .set_created_by("fabric-te".to_string())
        .build()
}

fn write_batch(path: &Path, schema: Arc<Schema>, cols: Vec<ArrayRef>) -> Result<(), TraceError> {
    let batch = RecordBatch::try_new(schema.clone(), cols)?;
    let file = File::create(path)?;
    let mut writer = ArrowWriter::try_new(file, schema, Some(props()))?;
    writer.write(&batch)?;
    writer.close()?;
    Ok(())
}

fn write_events(dir: &Path, rows: &[EventRow]) -> Result<(), TraceError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("t_ps", DataType::Int64, false),
        Field::new("seq", DataType::UInt64, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("epoch", DataType::UInt32, false),
        Field::new("job_id", DataType::UInt32, true),
        Field::new("flow_id", DataType::UInt64, true),
        Field::new("link_id", DataType::UInt32, true),
        Field::new("spine_id", DataType::UInt32, true),
        Field::new("leaf_id", DataType::UInt32, true),
        Field::new("rail_id", DataType::UInt8, true),
        Field::new("reject", DataType::Utf8, true),
        Field::new("bytes", DataType::UInt64, true),
    ]));
    let t_ps: Int64Array = rows.iter().map(|r| r.t_ps).collect();
    let seq: UInt64Array = rows.iter().map(|r| r.seq).collect();
    let kind: StringArray = rows.iter().map(|r| Some(r.kind.as_str())).collect();
    let epoch: UInt32Array = rows.iter().map(|r| r.epoch).collect();
    let job_id: UInt32Array = rows.iter().map(|r| r.job_id).collect();
    let flow_id: UInt64Array = rows.iter().map(|r| r.flow_id).collect();
    let link_id: UInt32Array = rows.iter().map(|r| r.link_id).collect();
    let spine_id: UInt32Array = rows.iter().map(|r| r.spine_id).collect();
    let leaf_id: UInt32Array = rows.iter().map(|r| r.leaf_id).collect();
    let rail_id: UInt8Array = rows.iter().map(|r| r.rail_id).collect();
    let reject: StringArray = rows.iter().map(|r| r.reject.as_deref()).collect();
    let bytes: UInt64Array = rows.iter().map(|r| r.bytes).collect();
    write_batch(
        &dir.join("events.parquet"),
        schema,
        vec![
            Arc::new(t_ps),
            Arc::new(seq),
            Arc::new(kind),
            Arc::new(epoch),
            Arc::new(job_id),
            Arc::new(flow_id),
            Arc::new(link_id),
            Arc::new(spine_id),
            Arc::new(leaf_id),
            Arc::new(rail_id),
            Arc::new(reject),
            Arc::new(bytes),
        ],
    )
}

fn write_flows(dir: &Path, rows: &[FlowRow]) -> Result<(), TraceError> {
    let item = Arc::new(Field::new("item", DataType::UInt32, true));
    let schema = Arc::new(Schema::new(vec![
        Field::new("flow_id", DataType::UInt64, false),
        Field::new("job_id", DataType::UInt32, false),
        Field::new("phase", DataType::UInt32, false),
        Field::new("src_gpu", DataType::UInt32, false),
        Field::new("dst_gpu", DataType::UInt32, false),
        Field::new("path_links", DataType::List(item), false),
        Field::new("rate_Bps", DataType::UInt64, false),
        Field::new("bytes", DataType::UInt64, false),
        Field::new("t_arrive_ps", DataType::Int64, false),
        Field::new("t_depart_ps", DataType::Int64, false),
    ]));
    let flow_id: UInt64Array = rows.iter().map(|r| r.flow_id).collect();
    let job_id: UInt32Array = rows.iter().map(|r| r.job_id).collect();
    let phase: UInt32Array = rows.iter().map(|r| r.phase).collect();
    let src_gpu: UInt32Array = rows.iter().map(|r| r.src_gpu).collect();
    let dst_gpu: UInt32Array = rows.iter().map(|r| r.dst_gpu).collect();
    let mut paths = ListBuilder::new(UInt32Builder::new());
    for r in rows {
        for &id in &r.path_links {
            paths.values().append_value(id);
        }
        paths.append(true);
    }
    let rate: UInt64Array = rows.iter().map(|r| r.rate_Bps).collect();
    let bytes: UInt64Array = rows.iter().map(|r| r.bytes).collect();
    let t_arr: Int64Array = rows.iter().map(|r| r.t_arrive_ps).collect();
    let t_dep: Int64Array = rows.iter().map(|r| r.t_depart_ps).collect();
    write_batch(
        &dir.join("flows.parquet"),
        schema,
        vec![
            Arc::new(flow_id),
            Arc::new(job_id),
            Arc::new(phase),
            Arc::new(src_gpu),
            Arc::new(dst_gpu),
            Arc::new(paths.finish()),
            Arc::new(rate),
            Arc::new(bytes),
            Arc::new(t_arr),
            Arc::new(t_dep),
        ],
    )
}

fn write_links(dir: &Path, rows: &[LinkRow]) -> Result<(), TraceError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("link_id", DataType::UInt32, false),
        Field::new("t_ps", DataType::Int64, false),
        Field::new("c_Bps", DataType::UInt64, false),
        Field::new("cir_Bps", DataType::UInt64, false),
        Field::new("r_avail_Bps", DataType::UInt64, false),
        Field::new("q_bytes", DataType::UInt64, false),
        Field::new("failed", DataType::UInt8, false),
    ]));
    let link_id: UInt32Array = rows.iter().map(|r| r.link_id).collect();
    let t_ps: Int64Array = rows.iter().map(|r| r.t_ps).collect();
    let c: UInt64Array = rows.iter().map(|r| r.c_Bps).collect();
    let cir: UInt64Array = rows.iter().map(|r| r.cir_Bps).collect();
    let rav: UInt64Array = rows.iter().map(|r| r.r_avail_Bps).collect();
    let q: UInt64Array = rows.iter().map(|r| r.q_bytes).collect();
    let failed: UInt8Array = rows
        .iter()
        .map(|r| if r.failed { 1u8 } else { 0 })
        .collect();
    write_batch(
        &dir.join("links.parquet"),
        schema,
        vec![
            Arc::new(link_id),
            Arc::new(t_ps),
            Arc::new(c),
            Arc::new(cir),
            Arc::new(rav),
            Arc::new(q),
            Arc::new(failed),
        ],
    )
}

fn write_jobs(dir: &Path, rows: &[JobRow]) -> Result<(), TraceError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("job_id", DataType::UInt32, false),
        Field::new("arrive_ps", DataType::Int64, false),
        Field::new("exit_ps", DataType::Int64, false),
        Field::new("decision", DataType::Utf8, false),
        Field::new("reject", DataType::Utf8, true),
        Field::new("binding_kind", DataType::Utf8, true),
        Field::new("t_pred_ps", DataType::Int64, false),
        Field::new("d_j_ps", DataType::Int64, false),
        Field::new("steps_done", DataType::UInt32, false),
    ]));
    let job_id: UInt32Array = rows.iter().map(|r| r.job_id).collect();
    let arrive: Int64Array = rows.iter().map(|r| r.arrive_ps).collect();
    let exit: Int64Array = rows.iter().map(|r| r.exit_ps).collect();
    let decision: StringArray = rows.iter().map(|r| Some(r.decision.as_str())).collect();
    let reject: StringArray = rows.iter().map(|r| r.reject.as_deref()).collect();
    let bind: StringArray = rows.iter().map(|r| r.binding_kind.as_deref()).collect();
    let t_pred: Int64Array = rows.iter().map(|r| r.t_pred_ps).collect();
    let d_j: Int64Array = rows.iter().map(|r| r.d_j_ps).collect();
    let steps: UInt32Array = rows.iter().map(|r| r.steps_done).collect();
    write_batch(
        &dir.join("jobs.parquet"),
        schema,
        vec![
            Arc::new(job_id),
            Arc::new(arrive),
            Arc::new(exit),
            Arc::new(decision),
            Arc::new(reject),
            Arc::new(bind),
            Arc::new(t_pred),
            Arc::new(d_j),
            Arc::new(steps),
        ],
    )
}

pub fn ps_i64(ps: i128) -> i64 {
    ps.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// I6: rollup parquet + admit.jsonl under `--out`.
pub fn rollup_dir(dir: &Path) -> Result<TraceRollup, TraceError> {
    let mut r = TraceRollup::default();
    r.arrivals = count_kind(dir, "events.parquet", "kind", "JobArrive")?;
    let jobs = string_col(dir, "jobs.parquet", "decision")?;
    for d in jobs {
        match d.as_str() {
            "kill" => r.kills = r.kills.saturating_add(1),
            "admit" => r.completes = r.completes.saturating_add(1),
            _ => {}
        }
    }
    let admit_path = dir.join("admit.jsonl");
    if admit_path.exists() {
        let s = fs::read_to_string(&admit_path)?;
        for line in s.lines().filter(|l| !l.trim().is_empty()) {
            let v: Value = serde_json::from_str(line).map_err(|e| TraceError(e.to_string()))?;
            match v.get("decision").and_then(|d| d.as_str()) {
                Some("admit") => r.admits = r.admits.saturating_add(1),
                Some("reject") => r.rejects = r.rejects.saturating_add(1),
                _ => {}
            }
        }
    }
    Ok(r)
}

fn count_kind(dir: &Path, file: &str, col: &str, want: &str) -> Result<u64, TraceError> {
    Ok(string_col(dir, file, col)?
        .into_iter()
        .filter(|k| k == want)
        .count() as u64)
}

fn string_col(dir: &Path, file: &str, col: &str) -> Result<Vec<String>, TraceError> {
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    let path = dir.join(file);
    let f = File::open(&path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(f)?;
    let reader = builder.build()?;
    let mut out = Vec::new();
    for batch in reader {
        let batch = batch?;
        let arr = batch
            .column_by_name(col)
            .ok_or_else(|| TraceError(format!("missing column {col}")))?;
        let s = arr
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| TraceError(format!("column {col} not utf8")))?;
        for i in 0..s.len() {
            if s.is_valid(i) {
                out.push(s.value(i).to_string());
            }
        }
    }
    Ok(out)
}
