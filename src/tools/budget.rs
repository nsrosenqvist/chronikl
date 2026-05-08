//! Per-run tool-call budget. Caps how many tool invocations a single
//! agent loop may make so a runaway model can't burn the whole turn
//! budget on tool churn.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone)]
pub struct ToolBudget {
    inner: Arc<BudgetInner>,
}

#[derive(Debug)]
struct BudgetInner {
    cap: usize,
    used: AtomicUsize,
}

impl ToolBudget {
    /// Build a budget. `cap == 0` means "no cap" — calls always succeed.
    pub fn new(cap: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(BudgetInner {
                cap,
                used: AtomicUsize::new(0),
            }),
        })
    }

    /// Try to consume one budget unit. Returns `Ok(used)` on success,
    /// `Err(cap)` when the budget is exhausted.
    pub fn try_consume(&self) -> Result<usize, usize> {
        if self.inner.cap == 0 {
            return Ok(self.inner.used.fetch_add(1, Ordering::SeqCst) + 1);
        }
        let prev = self.inner.used.fetch_add(1, Ordering::SeqCst);
        if prev >= self.inner.cap {
            // We over-consumed; roll back so subsequent calls see the
            // same value rather than letting the count drift unbounded.
            self.inner.used.fetch_sub(1, Ordering::SeqCst);
            Err(self.inner.cap)
        } else {
            Ok(prev + 1)
        }
    }

    pub fn used(&self) -> usize {
        self.inner.used.load(Ordering::SeqCst)
    }

    pub fn cap(&self) -> usize {
        self.inner.cap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_grants_within_cap() {
        let b = ToolBudget::new(3);
        assert_eq!(b.try_consume().unwrap(), 1);
        assert_eq!(b.try_consume().unwrap(), 2);
        assert_eq!(b.try_consume().unwrap(), 3);
    }

    #[test]
    fn budget_rejects_after_cap() {
        let b = ToolBudget::new(2);
        b.try_consume().unwrap();
        b.try_consume().unwrap();
        assert!(b.try_consume().is_err());
        assert_eq!(b.used(), 2, "should not over-count rejected calls");
    }

    #[test]
    fn zero_cap_means_unlimited() {
        let b = ToolBudget::new(0);
        for _ in 0..1000 {
            assert!(b.try_consume().is_ok());
        }
    }
}
