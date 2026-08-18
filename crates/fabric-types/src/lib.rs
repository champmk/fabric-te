//! Shared IDs, closed event set, and sim-time. Transcribed from docs/DESIGN.md §9.1–§9.2.
//!
//! One newtype per ID on purpose (DESIGN §5): `GpuId` ≠ `LinkId`.
//! Repeated derives are the cost. Do not `type GpuId = u32`.

#![forbid(unsafe_code)]

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct NodeId(pub u32);
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct GpuId(pub u32);
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct NicId(pub u32);
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct LeafId(pub u32);
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct SpineId(pub u32);
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct LinkId(pub u32);
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct RailId(pub u8);
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct JobId(pub u32);
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct FlowId(pub u64);
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct EpochId(pub u32);
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct Rank(pub u32);
/// Assigned at successful admit, dense 0..
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct AdmitSeq(pub u64);

/// Sim time. Never f64.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct SimTime {
    pub ps: i128,
    /// Monotonic insert counter; unique per `Fel::push`.
    pub seq: u64,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum CollectiveKind {
    RingAllReduce,
    PairwiseAllToAll,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Policy {
    Naive,
    Joint,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum BindingKind {
    NaiveFirstFit,
    FirstFitShift { skip_free_gpus: u8 },
    RailRotate { start_rail: u8 },
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum RejectCode {
    NoFreeGpus,
    FragmentedGpus,
    ResidualExhausted,
    SloMiss,
    CrossRailUnsupported,
    DeadElementOnPath,
    EpochPrepareFailed,
    MixDoesNotFit,
    OddRingDegenerate,
    ZeroLeftover,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(i32)]
pub enum ProcessExit {
    Ok = 0,
    Usage = 1,
    BadInput = 2,
    InvariantFail = 3,
    MixDoesNotFit = 4,
    IoAbort = 5,
}

/// Closed set of 14 FEL kinds. Admits/rejects/kills are traces, not events.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum EventKind {
    JobArrive,
    StepBoundary,
    CollectiveStart,
    CollectiveEnd,
    FlowArrive,
    FlowDepart,
    RateRecompute,
    LinkFail,
    LeafFail,
    RailFail,
    SpineFail,
    DrainComplete,
    EpochAdvance,
    HorizonCut,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum EventPayload {
    JobArrive { job: JobId },
    StepBoundary { job: JobId, step: u32 },
    CollectiveStart { job: JobId, step: u32 },
    CollectiveEnd { job: JobId, step: u32 },
    FlowArrive { flow: FlowId },
    FlowDepart { flow: FlowId },
    RateRecompute { reason: RecomputeReason },
    LinkFail { link: LinkId },
    LeafFail { leaf: LeafId },
    RailFail { rail: RailId },
    SpineFail { spine: SpineId },
    DrainComplete { job: JobId },
    EpochAdvance { from: EpochId, to: EpochId },
    HorizonCut,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum RecomputeReason {
    Admit(JobId),
    JobExit(JobId),
    EpochCommit,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Endpoint {
    Nic(NicId),
    Leaf(LeafId),
    Spine(SpineId),
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum GpuAvail {
    Present,
    Unavailable(UnavailReason),
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum UnavailReason {
    FailedNic,
    AbsentRow,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum JobState {
    Queued,
    Admitted,
    Computing,
    Collecting,
    Completed,
    Killed,
    Rejected,
}

impl EventKind {
    pub const fn is_fail(self) -> bool {
        matches!(
            self,
            EventKind::LinkFail | EventKind::LeafFail | EventKind::RailFail | EventKind::SpineFail
        )
    }
}

impl EventKind {
    /// Exhaustive count lock: 14 variants.
    pub const COUNT: usize = 14;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_closed_set_is_14() {
        let kinds = [
            EventKind::JobArrive,
            EventKind::StepBoundary,
            EventKind::CollectiveStart,
            EventKind::CollectiveEnd,
            EventKind::FlowArrive,
            EventKind::FlowDepart,
            EventKind::RateRecompute,
            EventKind::LinkFail,
            EventKind::LeafFail,
            EventKind::RailFail,
            EventKind::SpineFail,
            EventKind::DrainComplete,
            EventKind::EpochAdvance,
            EventKind::HorizonCut,
        ];
        assert_eq!(kinds.len(), EventKind::COUNT);
    }

    #[test]
    fn process_exit_codes_match_spec() {
        assert_eq!(ProcessExit::Ok as i32, 0);
        assert_eq!(ProcessExit::Usage as i32, 1);
        assert_eq!(ProcessExit::BadInput as i32, 2);
        assert_eq!(ProcessExit::InvariantFail as i32, 3);
        assert_eq!(ProcessExit::MixDoesNotFit as i32, 4);
        assert_eq!(ProcessExit::IoAbort as i32, 5);
    }
}
