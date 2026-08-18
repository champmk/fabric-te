//! Flow-level discrete-event kernel. PR1: clock + FEL only. No graph.

#![forbid(unsafe_code)]

pub mod fel;

pub use fabric_types::{EventKind, EventPayload, JobId, SimTime};
pub use fel::{Event, Fel};

/// Mix-file seconds → picoseconds. IEEE ties-to-even on the integer picosecond.
pub fn s_to_ps(x: f64) -> i128 {
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
}
