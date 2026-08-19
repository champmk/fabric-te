//! Static B&W `report.html`. Transcribed from docs/DESIGN.md §9.9, §16.5.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

use crate::Report;

pub fn write_html(report: &Report, path: &Path) -> Result<(), io::Error> {
    let mut s = String::new();
    s.push_str(
        "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <title>fabric-te report</title>\
         <style>body{background:#fff;color:#000;font-family:sans-serif}\
         table{border-collapse:collapse;margin:1em 0}\
         th,td{border:1px solid #000;padding:4px;text-align:left}\
         caption{caption-side:top;font-weight:bold;text-align:left}</style>\
         </head><body>\n",
    );
    s.push_str("<h1>Summary</h1>\n");
    table(
        &mut s,
        "Run summary",
        &["key", "value"],
        &[
            vec!["spec_version".into(), report.spec_version.clone()],
            vec!["seed".into(), report.seed.to_string()],
            vec!["policy".into(), report.policy.clone()],
            vec!["mix_hash".into(), report.mix_hash.clone()],
            vec!["topo_hash".into(), report.topo_hash.clone()],
            vec!["horizon_ps".into(), report.horizon_ps.to_string()],
            vec!["invariants_ok".into(), report.invariants_ok.to_string()],
            vec!["gpus".into(), report.topo.gpus.to_string()],
            vec!["N".into(), report.topo.N.to_string()],
            vec!["L".into(), report.topo.L.to_string()],
            vec!["S".into(), report.topo.S.to_string()],
            vec!["E_host".into(), report.topo.E_host.to_string()],
            vec!["E_ls".into(), report.topo.E_ls.to_string()],
            vec![
                "B_bisect_gbps".into(),
                report.topo.B_bisect_gbps.to_string(),
            ],
            vec!["arrivals".into(), report.counts.arrivals.to_string()],
            vec!["admits".into(), report.counts.admits.to_string()],
            vec!["rejects".into(), report.counts.rejects.to_string()],
            vec!["kills".into(), report.counts.kills.to_string()],
            vec!["completes".into(), report.counts.completes.to_string()],
            vec!["slo_misses".into(), report.counts.slo_misses.to_string()],
            vec!["hotspot_us".into(), report.metrics.hotspot_us.to_string()],
            vec![
                "hotspot_threshold_ppm".into(),
                report.metrics.hotspot_threshold_ppm.to_string(),
            ],
            vec![
                "completions_by_deadline".into(),
                report.metrics.completions_by_deadline.to_string(),
            ],
            vec![
                "tail_collective_us_p99".into(),
                report.metrics.tail_collective_us_p99.to_string(),
            ],
            vec![
                "last_flow_collective_us_max".into(),
                report.metrics.last_flow_collective_us_max.to_string(),
            ],
            vec!["slo_miss_us".into(), report.metrics.slo_miss_us.to_string()],
            vec![
                "disrupted_step_us".into(),
                report.metrics.disrupted_step_us.to_string(),
            ],
            vec![
                "mean_link_util_ppm".into(),
                report.metrics.mean_link_util_ppm.to_string(),
            ],
        ],
    );

    s.push_str("<h1>Jobs</h1>\n");
    let job_rows: Vec<Vec<String>> = report
        .jobs
        .iter()
        .map(|j| {
            vec![
                j.job_id.to_string(),
                j.decision.clone(),
                j.steps_done.to_string(),
                j.t_pred_ps.to_string(),
                j.reject.clone().unwrap_or_else(|| "-".into()),
            ]
        })
        .collect();
    table(
        &mut s,
        "Jobs",
        &["job_id", "decision", "steps_done", "t_pred_ps", "reject"],
        &job_rows,
    );

    s.push_str("<h1>Rejects</h1>\n");
    let rej: Vec<Vec<String>> = report
        .rejects_by_code
        .iter()
        .map(|(k, v)| vec![k.clone(), v.to_string()])
        .collect();
    table(&mut s, "Rejects by code", &["code", "count"], &rej);

    s.push_str("<h1>Hot links</h1>\n");
    table(
        &mut s,
        "Hot links",
        &["note"],
        &[vec!["See links.parquet for per-link snapshots.".into()]],
    );

    s.push_str("<h1>Failures</h1>\n");
    let fail_rows: Vec<Vec<String>> = if report.fails.is_empty() {
        vec![vec!["none".into()]]
    } else {
        report.fails.iter().map(|v| vec![v.to_string()]).collect()
    };
    table(&mut s, "Failures", &["fail"], &fail_rows);

    s.push_str("<h1>Planner</h1>\n");
    if let Some(p) = &report.plan {
        let extra = p
            .restore
            .extra_spines
            .map(|n| n.to_string())
            .unwrap_or_else(|| "null".into());
        table(
            &mut s,
            "Planner",
            &["key", "value"],
            &[
                vec!["deltas".into(), p.deltas.join(", ")],
                vec![
                    "nodes_removed".into(),
                    p.nodes_removed
                        .iter()
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                ],
                vec!["gpus_removed".into(), p.gpus_removed.to_string()],
                vec!["S_before".into(), p.S_before.to_string()],
                vec!["S_after".into(), p.S_after.to_string()],
                vec![
                    "jobs_admitted".into(),
                    p.jobs_admitted
                        .iter()
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                ],
                vec![
                    "jobs_rejected".into(),
                    p.jobs_rejected
                        .iter()
                        .map(|j| format!("{}:{}", j.id, j.code))
                        .collect::<Vec<_>>()
                        .join(", "),
                ],
                vec![
                    "new_hotspots".into(),
                    p.new_hotspots
                        .iter()
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                ],
                vec!["restore.extra_spines".into(), extra],
                vec![
                    "restore.rows_needed".into(),
                    p.restore.rows_needed.join(", "),
                ],
                vec![
                    "vs_baseline.admits".into(),
                    p.vs_baseline.admits.to_string(),
                ],
                vec![
                    "vs_baseline.rejects".into(),
                    p.vs_baseline.rejects.to_string(),
                ],
            ],
        );
    } else {
        table(
            &mut s,
            "Planner",
            &["note"],
            &[vec!["No planner delta in this run.".into()]],
        );
    }

    s.push_str("</body></html>\n");
    fs::write(path, s)
}

fn table(out: &mut String, caption: &str, headers: &[&str], rows: &[Vec<String>]) {
    let _ = writeln!(out, "<table><caption>{caption}</caption><thead><tr>");
    for h in headers {
        let _ = write!(out, "<th>{h}</th>");
    }
    let _ = writeln!(out, "</tr></thead><tbody>");
    for row in rows {
        let _ = write!(out, "<tr>");
        for cell in row {
            let _ = write!(out, "<td>{}</td>", escape(cell));
        }
        let _ = writeln!(out, "</tr>");
    }
    if rows.is_empty() {
        let _ = writeln!(out, "<tr><td colspan=\"{}\">none</td></tr>", headers.len());
    }
    let _ = writeln!(out, "</tbody></table>");
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
