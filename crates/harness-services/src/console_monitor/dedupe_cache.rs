//! Per-(hwnd, pid) dedupe of console window events (Python `recently_seen`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// `EVENT_SYSTEM_FOREGROUND`.
pub const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
/// `EVENT_OBJECT_SHOW`.
pub const EVENT_OBJECT_SHOW: u32 = 0x8002;
/// Repeat events for the same window within this window are dropped.
pub const DEDUPE_WINDOW: Duration = Duration::from_secs(2);
/// When the cache grows beyond this many entries it is cleared wholesale.
pub const MAX_ENTRIES: usize = 128;

/// Window handle (as an integer) and owning process id.
pub type WindowKey = (usize, u32);

/// Remembers when each window was last reported.
#[derive(Debug, Default)]
pub struct DedupeCache {
    last_seen: HashMap<WindowKey, Instant>,
}

impl DedupeCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.last_seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.last_seen.is_empty()
    }

    /// Decides whether this event should proceed, recording it when it does.
    ///
    /// Rules (same order as Python):
    /// 1. a foreground event for an already-seen window is dropped (foreground-seen rule);
    /// 2. any event within [`DEDUPE_WINDOW`] of the previous one for the same window is dropped;
    /// 3. otherwise the time is recorded and the cache is cleared once it exceeds [`MAX_ENTRIES`].
    pub fn admit(&mut self, key: WindowKey, event: u32, now: Instant) -> bool {
        if event == EVENT_SYSTEM_FOREGROUND && self.last_seen.contains_key(&key) {
            return false;
        }
        if self.seen_within_window(key, now) {
            return false;
        }
        self.last_seen.insert(key, now);
        if self.last_seen.len() > MAX_ENTRIES {
            self.last_seen.clear();
        }
        true
    }

    #[inline]
    fn seen_within_window(&self, key: WindowKey, now: Instant) -> bool {
        self.last_seen
            .get(&key)
            .is_some_and(|previous| now.saturating_duration_since(*previous) < DEDUPE_WINDOW)
    }
}
