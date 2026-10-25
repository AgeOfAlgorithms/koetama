//! What the host's LLM has cost this month: ONE count for everything that asks it (the agents, the window's "Try it"),
//! shared (Arc), and what requests under way may still cost (held before each is sent: several at once cannot all slip
//! under the budget and overshoot it together).
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
pub struct Ledger(Mutex<(f64, f64)>);

impl Ledger {
    /// A ledger that has spent `spent` dollars already (this month's, as the setting kept it).
    pub fn new(spent: f64) -> Arc<Ledger> {
        Arc::new(Ledger(Mutex::new((spent.max(0.0), 0.0))))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (f64, f64)> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Dollars spent so far.
    pub fn spent(&self) -> f64 {
        self.lock().0
    }

    /// What a request outside the agents cost (the window's "Try it").
    pub fn add(&self, cost: f64) {
        self.lock().0 += cost.max(0.0);
    }

    /// A new month: what was spent starts over.
    pub fn set(&self, spent: f64) {
        self.lock().0 = spent.max(0.0);
    }

    /// Holds `most` (what a request could cost at worst) if what was spent, what is held and it stay within `budget`;
    /// false: the request must not go.
    pub fn reserve(&self, most: f64, budget: f64) -> bool {
        let mut g = self.lock();
        if g.0 + g.1 + most > budget {
            return false;
        }
        g.1 += most;
        true
    }

    /// A request held with `most` came back having cost `cost` (0: it failed).
    pub fn settle(&self, most: f64, cost: f64) {
        let mut g = self.lock();
        g.1 = (g.1 - most).max(0.0);
        g.0 += cost.max(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_in_flight_cannot_overshoot_together() {
        let l = Ledger::new(4.0);
        assert!(l.reserve(0.6, 5.0));
        assert!(!l.reserve(0.6, 5.0), "a second at once would pass the budget: held back");
        l.settle(0.6, 0.1);
        assert!((l.spent() - 4.1).abs() < 1e-9);
        assert!(l.reserve(0.6, 5.0), "the first one's hold is let go");
        l.settle(0.6, 0.0);
        l.add(0.5);
        assert!((l.spent() - 4.6).abs() < 1e-9);
        l.set(0.0);
        assert_eq!(l.spent(), 0.0);
    }
}
