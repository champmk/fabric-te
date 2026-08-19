//! Naive admit, occupancy, job table, run kernel, joint evaluate/admit, planner.

#![forbid(unsafe_code)]
#![allow(non_snake_case)]

mod epoch;
mod inv;
mod joint;
mod kernel;
mod naive;
mod plan;
mod table;

pub use epoch::{parse_fail_spec, prepare, EpochPlan, FailKind, FailSpec};
pub use inv::{
    first_mutate_broken, i10_holds, i4_holds, i5_holds, i6_holds, i7_holds, LiveCounters,
    TraceRollup,
};
pub use joint::{evaluate, example_c, generate_bindings, joint_admit, select_code, Feasible};
pub use kernel::{run_sim, run_sim_snapshot, RunConfig, RunError, RunSnapshot};
pub use naive::{first_fit, gpu_scan_order, naive_admit, pump_until_collective_start};
pub use plan::{apply_deltas, parse_delta, run_plan, Delta, PlanConfig, PlanError, PlanOutcome};
pub use table::{
    communicators, rank_map, Binding, BindingNote, Communicator, Flow, JobRec, JobTable, Occupancy,
};
