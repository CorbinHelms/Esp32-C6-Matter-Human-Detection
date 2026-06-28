//! Presence debouncing.
//!
//! The LD2410 reports continuously (tens of milliseconds apart) and its raw
//! "someone present" bit can briefly drop even while a still person is in the
//! room. Mapping that raw bit straight onto a Matter Occupancy attribute would
//! make Google Home flap between occupied/unoccupied. [`PresenceDebouncer`]
//! smooths it with a *hold time*: presence asserts immediately, but only clears
//! after the raw signal has stayed absent continuously for `hold_ms`.
//!
//! The logic is intentionally free of any HAL/embassy types — it works purely
//! on millisecond timestamps — so it is simple to reason about and to unit
//! test in isolation.

/// Debounces a raw presence bit into a stable occupied/unoccupied state.
#[derive(Debug, Clone)]
pub struct PresenceDebouncer {
    hold_ms: u64,
    present: bool,
    /// Timestamp of the most recent raw detection.
    last_raw_present_ms: u64,
}

impl PresenceDebouncer {
    /// Creates a debouncer that clears presence after `hold_ms` of continuous
    /// absence. Starts in the unoccupied state.
    pub const fn new(hold_ms: u64) -> Self {
        Self {
            hold_ms,
            present: false,
            last_raw_present_ms: 0,
        }
    }

    /// Feeds a raw detection sampled at `now_ms`.
    ///
    /// Returns `Some(new_state)` only when the debounced presence *changes*,
    /// so callers can publish/log transitions rather than every sample.
    pub fn update(&mut self, raw_present: bool, now_ms: u64) -> Option<bool> {
        if raw_present {
            self.last_raw_present_ms = now_ms;
            if !self.present {
                self.present = true;
                return Some(true);
            }
        } else if self.present && now_ms.saturating_sub(self.last_raw_present_ms) >= self.hold_ms {
            self.present = false;
            return Some(false);
        }
        None
    }

    /// The current debounced presence state.
    pub fn is_present(&self) -> bool {
        self.present
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asserts_immediately_and_holds_before_clearing() {
        // The hold window is measured from the *last* raw detection.
        let mut d = PresenceDebouncer::new(2000);
        assert_eq!(d.update(true, 0), Some(true)); // first detection -> occupied
        assert_eq!(d.update(true, 500), None); // still present, hold ref -> 500
        assert_eq!(d.update(false, 1000), None); // 500ms absent, inside window
        assert_eq!(d.update(false, 2499), None); // 1999ms < 2000ms hold
        assert_eq!(d.update(false, 2500), Some(false)); // 2000ms since t=500 -> clear
        assert!(!d.is_present());
    }

    #[test]
    fn renewed_presence_resets_hold_window() {
        let mut d = PresenceDebouncer::new(1000);
        assert_eq!(d.update(true, 0), Some(true));
        assert_eq!(d.update(false, 500), None);
        assert_eq!(d.update(true, 800), None); // re-detected, stays occupied
        assert_eq!(d.update(false, 1700), None); // only 900ms since last detection
        assert_eq!(d.update(false, 1800), Some(false)); // 1000ms since detection at 800
    }
}
