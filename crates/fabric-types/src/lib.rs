//! Shared IDs, closed event set, and sim-time. Transcribed from docs/DESIGN.md §9.1–§9.2.

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

    /// Exhaustive count lock: 14 variants.
    pub const COUNT: usize = 14;

    pub const fn as_str(self) -> &'static str {
        match self {
            EventKind::JobArrive => "JobArrive",
            EventKind::StepBoundary => "StepBoundary",
            EventKind::CollectiveStart => "CollectiveStart",
            EventKind::CollectiveEnd => "CollectiveEnd",
            EventKind::FlowArrive => "FlowArrive",
            EventKind::FlowDepart => "FlowDepart",
            EventKind::RateRecompute => "RateRecompute",
            EventKind::LinkFail => "LinkFail",
            EventKind::LeafFail => "LeafFail",
            EventKind::RailFail => "RailFail",
            EventKind::SpineFail => "SpineFail",
            EventKind::DrainComplete => "DrainComplete",
            EventKind::EpochAdvance => "EpochAdvance",
            EventKind::HorizonCut => "HorizonCut",
        }
    }

    /// EventKind / EventPayload 1:1. Mismatch is I3-class (`E_INV`).
    pub const fn matches_payload(self, payload: &EventPayload) -> bool {
        matches!(
            (self, payload),
            (EventKind::JobArrive, EventPayload::JobArrive { .. })
                | (EventKind::StepBoundary, EventPayload::StepBoundary { .. })
                | (
                    EventKind::CollectiveStart,
                    EventPayload::CollectiveStart { .. }
                )
                | (EventKind::CollectiveEnd, EventPayload::CollectiveEnd { .. })
                | (EventKind::FlowArrive, EventPayload::FlowArrive { .. })
                | (EventKind::FlowDepart, EventPayload::FlowDepart { .. })
                | (EventKind::RateRecompute, EventPayload::RateRecompute { .. })
                | (EventKind::LinkFail, EventPayload::LinkFail { .. })
                | (EventKind::LeafFail, EventPayload::LeafFail { .. })
                | (EventKind::RailFail, EventPayload::RailFail { .. })
                | (EventKind::SpineFail, EventPayload::SpineFail { .. })
                | (EventKind::DrainComplete, EventPayload::DrainComplete { .. })
                | (EventKind::EpochAdvance, EventPayload::EpochAdvance { .. })
                | (EventKind::HorizonCut, EventPayload::HorizonCut)
        )
    }
}

impl RejectCode {
    pub const ALL: [RejectCode; 10] = [
        RejectCode::NoFreeGpus,
        RejectCode::FragmentedGpus,
        RejectCode::ResidualExhausted,
        RejectCode::SloMiss,
        RejectCode::CrossRailUnsupported,
        RejectCode::DeadElementOnPath,
        RejectCode::EpochPrepareFailed,
        RejectCode::MixDoesNotFit,
        RejectCode::OddRingDegenerate,
        RejectCode::ZeroLeftover,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            RejectCode::NoFreeGpus => "NoFreeGpus",
            RejectCode::FragmentedGpus => "FragmentedGpus",
            RejectCode::ResidualExhausted => "ResidualExhausted",
            RejectCode::SloMiss => "SloMiss",
            RejectCode::CrossRailUnsupported => "CrossRailUnsupported",
            RejectCode::DeadElementOnPath => "DeadElementOnPath",
            RejectCode::EpochPrepareFailed => "EpochPrepareFailed",
            RejectCode::MixDoesNotFit => "MixDoesNotFit",
            RejectCode::OddRingDegenerate => "OddRingDegenerate",
            RejectCode::ZeroLeftover => "ZeroLeftover",
        }
    }
}

impl Policy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Policy::Naive => "naive",
            Policy::Joint => "joint",
        }
    }
}

impl BindingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BindingKind::NaiveFirstFit => "NaiveFirstFit",
            BindingKind::FirstFitShift { .. } => "FirstFitShift",
            BindingKind::RailRotate { .. } => "RailRotate",
        }
    }
}

/// Integer µs: floor(ps / 1_000_000). Never /1000.
pub fn ps_to_us(ps: i128) -> i128 {
    ps / 1_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_closed_set_is_14() {
        let n = exhaustive_kind_count(EventKind::JobArrive);
        assert_eq!(n, EventKind::COUNT);
    }

    fn exhaustive_kind_count(k: EventKind) -> usize {
        match k {
            EventKind::JobArrive
            | EventKind::StepBoundary
            | EventKind::CollectiveStart
            | EventKind::CollectiveEnd
            | EventKind::FlowArrive
            | EventKind::FlowDepart
            | EventKind::RateRecompute
            | EventKind::LinkFail
            | EventKind::LeafFail
            | EventKind::RailFail
            | EventKind::SpineFail
            | EventKind::DrainComplete
            | EventKind::EpochAdvance
            | EventKind::HorizonCut => EventKind::COUNT,
        }
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
