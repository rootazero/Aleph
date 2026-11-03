//! Flow catalog with atomic hot reload. See design §3.8, §5.

use crate::sync_primitives::Arc;
use std::collections::HashMap;

use arc_swap::ArcSwap;

use crate::orchestrator::flow_spec::{FlowId, FlowSpec};

pub type FlowSet = HashMap<FlowId, Arc<FlowSpec>>;

pub struct FlowRegistry {
    flows: ArcSwap<FlowSet>,
}

/// Hand-written so the registry stays printable in a log line without
/// dumping the whole catalog: the struct name plus the number of flows is
/// what an operator needs to tell "the catalog is empty" from "the catalog
/// has entries".
impl std::fmt::Debug for FlowRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowRegistry")
            .field("flows", &self.len())
            .finish()
    }
}

impl FlowRegistry {
    #[must_use]
    pub fn new(initial: FlowSet) -> Self {
        Self {
            flows: ArcSwap::from(Arc::new(initial)),
        }
    }

    /// Returns an `Arc<FlowSpec>` snapshot. In-flight dispatches keep
    /// this snapshot alive even after `replace()`.
    pub fn resolve(&self, id: &str) -> Option<Arc<FlowSpec>> {
        self.flows.load().get(id).cloned()
    }

    /// Atomic whole-catalog swap. Callers hold old snapshots until they drop them.
    pub fn replace(&self, new_set: FlowSet) {
        self.flows.store(Arc::new(new_set));
    }

    pub fn len(&self) -> usize {
        self.flows.load().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
