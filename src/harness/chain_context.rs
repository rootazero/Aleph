//! Chain Context — tracks subagent call chain nesting.
//!
//! `ChainContext` is an immutable struct that identifies an AgentLoop's
//! position in a call chain. It prevents infinite recursion by enforcing
//! a maximum depth limit.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn generate_chain_id() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("ch-{ts:x}-{seq:x}")
}

const DEFAULT_MAX_DEPTH: u32 = 5;

/// Tracks an AgentLoop's position in a subagent call chain.
///
/// Each top-level request gets a unique `chain_id`. Subagent calls
/// increment `depth`. When `depth >= max_depth`, `child()` returns
/// `None` to prevent infinite recursion.
#[derive(Clone, Debug)]
pub struct ChainContext {
    /// Unique identifier for this call chain (shared across all depths).
    pub chain_id: String,
    /// Current nesting depth. 0 = root agent.
    pub depth: u32,
    /// Maximum allowed depth before refusing to spawn children.
    pub max_depth: u32,
}

impl ChainContext {
    /// Create a root context with a generated chain_id, depth=0, max_depth=5.
    pub fn new() -> Self {
        Self {
            chain_id: generate_chain_id(),
            depth: 0,
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }

    /// Create a root context with a custom max_depth.
    pub fn with_max_depth(max_depth: u32) -> Self {
        Self {
            chain_id: generate_chain_id(),
            depth: 0,
            max_depth,
        }
    }

    /// Create a child context with depth+1 and the same chain_id.
    ///
    /// Returns `None` if the current depth has reached max_depth,
    /// preventing infinite recursion.
    pub fn child(&self) -> Option<ChainContext> {
        if self.depth >= self.max_depth {
            return None;
        }
        Some(ChainContext {
            chain_id: self.chain_id.clone(),
            depth: self.depth + 1,
            max_depth: self.max_depth,
        })
    }

    /// Returns true if this is a root-level context (depth == 0).
    pub fn is_root(&self) -> bool {
        self.depth == 0
    }
}

impl Default for ChainContext {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ChainContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ChainContext(chain_id={}, depth={}/{})",
            self.chain_id, self.depth, self.max_depth
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_top_level() {
        let ctx = ChainContext::new();
        assert_eq!(ctx.depth, 0);
        assert_eq!(ctx.max_depth, 5);
        assert!(!ctx.chain_id.is_empty());
        assert!(ctx.is_root());
    }

    #[test]
    fn child_increments_depth() {
        let root = ChainContext::new();
        let child = root.child().expect("child should be Some");
        assert_eq!(child.depth, 1);
        assert_eq!(child.chain_id, root.chain_id);
        assert_eq!(child.max_depth, root.max_depth);
        assert!(!child.is_root());
    }

    #[test]
    fn child_chain_preserves_through_levels() {
        let root = ChainContext::new();
        let c1 = root.child().unwrap();
        let c2 = c1.child().unwrap();
        let c3 = c2.child().unwrap();
        assert_eq!(c3.depth, 3);
        assert_eq!(c3.chain_id, root.chain_id);
    }

    #[test]
    fn child_returns_none_at_max_depth() {
        let root = ChainContext::with_max_depth(2);
        let c1 = root.child().expect("0→1 should succeed");
        assert_eq!(c1.depth, 1);
        let c2 = c1.child().expect("1→2 should succeed");
        assert_eq!(c2.depth, 2);
        let c3 = c2.child();
        assert!(c3.is_none(), "2→3 should be None (max_depth=2)");
    }

    #[test]
    fn unique_chain_ids() {
        let a = ChainContext::new();
        let b = ChainContext::new();
        assert_ne!(a.chain_id, b.chain_id);
    }

    #[test]
    fn display_format() {
        let ctx = ChainContext::new();
        let display = format!("{ctx}");
        assert!(
            display.contains(&ctx.chain_id),
            "Display should contain chain_id"
        );
        assert!(display.contains("depth=0"), "Display should contain depth");
    }
}
