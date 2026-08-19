//! Flow-level discrete-event kernel. FEL + residual + Clos paths.

#![forbid(unsafe_code)]

pub mod fel;
pub mod paths;
pub mod residual;
pub mod waterfill;

pub use fabric_types::{EventKind, EventPayload, JobId, SimTime};
pub use fel::{Event, Fel};
pub use paths::{k_shortest, Path, PathMode};
pub use residual::Residual;
pub use waterfill::water_fill;

/// Mix-file seconds → picoseconds. IEEE ties-to-even on the integer picosecond.
/// Non-finite input is rejected (NaN/Inf must not become t=0).
pub fn s_to_ps(x: f64) -> i128 {
    assert!(x.is_finite(), "s_to_ps: non-finite");
    (x * 1e12).round_ties_even() as i128
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_types::{EventKind, EventPayload, JobId};

    #[test]
    fn clock_ps_total_order() {
        let mut fel = Fel::new();
        fel.push(
            100,
            EventKind::JobArrive,
            EventPayload::JobArrive { job: JobId(1) },
        );
        fel.push(
            50,
            EventKind::JobArrive,
            EventPayload::JobArrive { job: JobId(2) },
        );
        fel.push(100, EventKind::HorizonCut, EventPayload::HorizonCut);

        let first = fel.pop().expect("50ps");
        assert_eq!(first.t.ps, 50);
        assert_eq!(first.t.seq, 1);

        let second = fel.pop().expect("100ps seq0");
        assert_eq!(second.t.ps, 100);
        assert_eq!(second.t.seq, 0);
        assert_eq!(second.payload, EventPayload::JobArrive { job: JobId(1) });

        let third = fel.pop().expect("100ps seq2");
        assert_eq!(third.t.ps, 100);
        assert_eq!(third.t.seq, 2);
        assert_eq!(third.kind, EventKind::HorizonCut);

        assert!(fel.pop().is_none());
        assert_eq!(s_to_ps(1.0), 1_000_000_000_000);
        assert_eq!(s_to_ps(0.0), 0);
    }

    #[test]
    fn fel_fires_one_event() {
        let mut fel = Fel::new();
        fel.push(
            0,
            EventKind::JobArrive,
            EventPayload::JobArrive { job: JobId(7) },
        );
        let e = fel.pop().expect("one event");
        assert_eq!(e.t.ps, 0);
        assert_eq!(e.t.seq, 0);
        assert_eq!(e.kind, EventKind::JobArrive);
        assert_eq!(e.payload, EventPayload::JobArrive { job: JobId(7) });
        assert!(fel.pop().is_none());
        assert!(fel.peek_ps().is_none());
    }

    #[test]
    fn s_to_ps_ties_to_even() {
        assert_eq!(s_to_ps(0.5e-12), 0);
        assert_eq!(s_to_ps(1.5e-12), 2);
    }

    #[test]
    #[should_panic(expected = "non-finite")]
    fn s_to_ps_rejects_nan() {
        let _ = s_to_ps(f64::NAN);
    }

    #[test]
    fn drain_fails_at_keeps_non_fails_and_seq() {
        use fabric_types::{LeafId, LinkId};

        let mut fel = Fel::new();
        fel.push(
            10,
            EventKind::JobArrive,
            EventPayload::JobArrive { job: JobId(1) },
        );
        fel.push(
            10,
            EventKind::LinkFail,
            EventPayload::LinkFail { link: LinkId(0) },
        );
        fel.push(
            10,
            EventKind::LeafFail,
            EventPayload::LeafFail { leaf: LeafId(0) },
        );
        fel.push(20, EventKind::HorizonCut, EventPayload::HorizonCut);

        let fails = fel.drain_fails_at(10);
        assert_eq!(fails.len(), 2);
        assert!(fails.iter().all(|e| e.kind.is_fail()));
        assert_eq!(fails[0].t.seq, 1);
        assert_eq!(fails[1].t.seq, 2);

        let kept = fel.pop().expect("non-fail at 10");
        assert_eq!(kept.kind, EventKind::JobArrive);
        assert_eq!(kept.t.ps, 10);
        assert_eq!(kept.t.seq, 0);

        let later = fel.pop().expect("20ps");
        assert_eq!(later.t.ps, 20);
        assert!(fel.pop().is_none());
    }
}
