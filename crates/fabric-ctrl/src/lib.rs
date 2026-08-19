//! Naive admit, occupancy, job table, run kernel, and joint bindings.

#![forbid(unsafe_code)]
#![allow(non_snake_case)]

mod joint;
mod kernel;
mod naive;
mod table;

pub use joint::generate_bindings;
pub use kernel::{run_sim, RunConfig, RunError};
pub use naive::{first_fit, gpu_scan_order, naive_admit, pump_until_collective_start};
pub use table::{
    communicators, rank_map, Binding, Communicator, Flow, JobRec, JobTable, Occupancy,
};
