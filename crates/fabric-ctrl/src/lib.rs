//! Naive admit, occupancy, job table, and run kernel. Transcribed from docs/DESIGN.md §9.4, §10, §12.

#![forbid(unsafe_code)]
#![allow(non_snake_case)]

mod kernel;
mod naive;
mod table;

pub use kernel::{run_sim, RunConfig, RunError};
pub use naive::{first_fit, gpu_scan_order, naive_admit, pump_until_collective_start};
pub use table::{
    communicators, rank_map, Binding, Communicator, Flow, JobRec, JobTable, Occupancy,
};
