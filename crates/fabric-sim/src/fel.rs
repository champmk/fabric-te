//! Future-event list. Transcribed from docs/DESIGN.md §9.4 and §10.1.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use fabric_types::{EventKind, EventPayload, SimTime};

/// FEL event. Order is `(ps asc, seq asc)` only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Event {
    pub t: SimTime,
    pub kind: EventKind,
    pub payload: EventPayload,
}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Event {
    fn cmp(&self, other: &Self) -> Ordering {
        self.t
            .ps
            .cmp(&other.t.ps)
            .then(self.t.seq.cmp(&other.t.seq))
    }
}

pub struct Fel {
    heap: BinaryHeap<Reverse<Event>>,
    next_seq: u64,
}

impl Fel {
    pub fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            next_seq: 0,
        }
    }

    /// Schedule at `ps`. `seq` is `next_seq`, then increment.
    pub fn push(&mut self, ps: i128, kind: EventKind, payload: EventPayload) {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.checked_add(1).expect("Fel seq overflow");
        self.insert(Event {
            t: SimTime { ps, seq },
            kind,
            payload,
        });
    }

    pub fn pop(&mut self) -> Option<Event> {
        self.heap.pop().map(|Reverse(e)| e)
    }

    pub fn peek_ps(&self) -> Option<i128> {
        self.heap.peek().map(|Reverse(e)| e.t.ps)
    }

    /// Seq assigned to the most recent `push`.
    pub fn last_push_seq(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }

    /// Coalesce Fail* at `ps`. Other events at `ps` stay, with their original seq.
    pub fn drain_fails_at(&mut self, ps: i128) -> Vec<Event> {
        let mut fails = Vec::new();
        let mut rest = Vec::new();
        while self.peek_ps() == Some(ps) {
            let e = self.pop().expect("peeked");
            if e.kind.is_fail() {
                fails.push(e);
            } else {
                rest.push(e);
            }
        }
        for e in rest {
            self.insert(e);
        }
        fails
    }

    fn insert(&mut self, ev: Event) {
        self.heap.push(Reverse(ev));
    }
}

impl Default for Fel {
    fn default() -> Self {
        Self::new()
    }
}
