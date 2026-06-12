//! `divan-trace` — trace timeline query/render models (K10, spec §5.3).
//!
//! Faz 1 needs only the query DTOs used to render `divan log`; the full TUI
//! timeline and metrics aggregation are Faz 4 (impl plan §5.7, §F4.1). Persisted
//! trace rows live in `divan-db`'s `TraceStore`; this crate shapes them for
//! presentation.

use divan_core::trace::TraceEvent;
use serde::{Deserialize, Serialize};

/// A flat, time-ordered view of one trace's events for `divan log`/`divan trace`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timeline {
    pub trace_id: String,
    pub events: Vec<TraceEvent>,
}

impl Timeline {
    pub fn new(trace_id: impl Into<String>, mut events: Vec<TraceEvent>) -> Self {
        events.sort_by_key(|e| e.ts);
        Self {
            trace_id: trace_id.into(),
            events,
        }
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use divan_core::trace::TraceEventKind;
    use divan_core::TraceId;

    #[test]
    fn timeline_sorts_by_ts() {
        let evs = vec![
            TraceEvent::new(TraceId::new("t"), TraceEventKind::TaskTransition, 30),
            TraceEvent::new(TraceId::new("t"), TraceEventKind::TaskCreated, 10),
        ];
        let tl = Timeline::new("t", evs);
        assert_eq!(tl.events[0].ts, 10);
        assert_eq!(tl.events[1].ts, 30);
    }
}
