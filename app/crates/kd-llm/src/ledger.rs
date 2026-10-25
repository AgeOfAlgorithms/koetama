//! What the host's LLM has used this month, in tokens in and out: ONE count for everything that asks it (the agents,
//! the window's "Try it"), shared (Arc), and what requests under way may still use (held before each is sent: several
//! at once cannot all slip under the limit and pass it together). The limit is in tokens, not dollars (the user,
//! 2026-10-25: "limit the usage by number of in/out tokens then? not by price" - a price could only be guessed).
use std::sync::{Arc, Mutex};

/// Tokens in (the prompt: cached ones too, as providers count them) and out (the reply).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
}

impl Tokens {
    pub fn new(input: u64, output: u64) -> Tokens {
        Tokens { input, output }
    }

    fn plus(self, o: Tokens) -> Tokens {
        Tokens::new(self.input.saturating_add(o.input), self.output.saturating_add(o.output))
    }

    fn minus(self, o: Tokens) -> Tokens {
        Tokens::new(self.input.saturating_sub(o.input), self.output.saturating_sub(o.output))
    }

    /// Both within `limit`.
    pub fn within(self, limit: Tokens) -> bool {
        self.input <= limit.input && self.output <= limit.output
    }

    /// Either at or past `limit`: nothing more can go.
    pub fn reached(self, limit: Tokens) -> bool {
        self.input >= limit.input || self.output >= limit.output
    }
}

/// (used, held)
#[derive(Debug, Default)]
pub struct Ledger(Mutex<(Tokens, Tokens)>);

impl Ledger {
    /// A ledger that has used `used` already (this month's, as the setting kept it).
    pub fn new(used: Tokens) -> Arc<Ledger> {
        Arc::new(Ledger(Mutex::new((used, Tokens::default()))))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (Tokens, Tokens)> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Tokens used so far.
    pub fn used(&self) -> Tokens {
        self.lock().0
    }

    /// What a request outside the agents used (the window's "Try it").
    pub fn add(&self, t: Tokens) {
        let mut g = self.lock();
        g.0 = g.0.plus(t);
    }

    /// A new month, or the host reset the count.
    pub fn set(&self, used: Tokens) {
        self.lock().0 = used;
    }

    /// Holds `most` (what a request could use at worst) if what was used, what is held and it stay within `limit`;
    /// false: the request must not go.
    pub fn reserve(&self, most: Tokens, limit: Tokens) -> bool {
        let mut g = self.lock();
        if !g.0.plus(g.1).plus(most).within(limit) {
            return false;
        }
        g.1 = g.1.plus(most);
        true
    }

    /// A request held with `most` came back having used `got` (nothing: it failed).
    pub fn settle(&self, most: Tokens, got: Tokens) {
        let mut g = self.lock();
        g.1 = g.1.minus(most);
        g.0 = g.0.plus(got);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_in_flight_cannot_overshoot_together() {
        let limit = Tokens::new(10_000, 1_000);
        let l = Ledger::new(Tokens::new(8_000, 100));
        let most = Tokens::new(1_500, 300);
        assert!(l.reserve(most, limit));
        assert!(!l.reserve(most, limit), "a second at once would pass the limit: held back");
        l.settle(most, Tokens::new(1_200, 40));
        assert_eq!(l.used(), Tokens::new(9_200, 140));
        assert!(!l.reserve(most, limit), "9,200 + 1,500 in would pass 10,000");
        assert!(l.reserve(Tokens::new(500, 300), limit));
        l.settle(Tokens::new(500, 300), Tokens::default());
        l.add(Tokens::new(800, 10));
        assert!(l.used().reached(limit), "10,000 in: reached");
        l.set(Tokens::default());
        assert_eq!(l.used(), Tokens::default());
    }
}
