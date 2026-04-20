//! Flow catalog with atomic hot reload. See design §3.8, §5.

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::orchestrator::flow_spec::{FlowId, FlowSpec};

pub type FlowSet = HashMap<FlowId, Arc<FlowSpec>>;

pub struct FlowRegistry {
    flows: ArcSwap<FlowSet>,
}

impl FlowRegistry {
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

    pub fn list_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.flows.load().keys().cloned().collect();
        ids.sort();
        ids
    }

    pub fn len(&self) -> usize {
        self.flows.load().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
