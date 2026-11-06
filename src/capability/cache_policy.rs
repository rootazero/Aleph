//! Bounded resolver metadata retention; never an authorization authority.

use crate::capability::descriptor::{CapabilityDescriptor, CapabilityId, CapabilityRevision};
use crate::capability::ownership::VisibilityScope;
use crate::sync_primitives::Arc;
use lru::LruCache;
use std::num::NonZeroUsize;

/// Eviction order for the resolver's single metadata cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EvictionPolicy {
    /// Reads keep recently accessed entries resident.
    #[default]
    Lru,
    /// Reads do not promote entries; replacement counts as a new insertion.
    Fifo,
}

/// Per-resolver bounds, independent of provider process/runtime budgets.
#[derive(Debug, Clone, Copy)]
pub struct ResolverConfig {
    /// Maximum retained (ID, exact visibility) pairs; zero disables caching.
    /// This is an entry-count limit, not a heap-byte limit.
    pub max_entries: usize,
    pub eviction: EvictionPolicy,
    /// Maximum overlapping discovery, lookup, or resolve calls. Zero disables
    /// new work. Saturation fails closed without allocating a waiting queue.
    pub max_concurrent_loads: u16,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            max_entries: 256,
            eviction: EvictionPolicy::Lru,
            max_concurrent_loads: 32,
        }
    }
}

pub(super) type CacheKey = (CapabilityId, VisibilityScope);

pub(super) struct ResolverCache {
    pub(super) descriptors: Option<LruCache<CacheKey, CapabilityDescriptor>>,
    eviction: EvictionPolicy,
    // Identity, not a wrapping counter. In-flight insertions crossing an
    // invalidation are discarded without retaining per-ID tombstones.
    epoch: Arc<()>,
}

impl ResolverCache {
    pub(super) fn new(config: ResolverConfig) -> Self {
        Self {
            descriptors: NonZeroUsize::new(config.max_entries).map(LruCache::new),
            eviction: config.eviction,
            epoch: Arc::new(()),
        }
    }

    pub(super) fn epoch(&self) -> Arc<()> {
        self.epoch.clone()
    }

    pub(super) fn get(
        &mut self,
        key: &CacheKey,
        revision: CapabilityRevision,
    ) -> Option<CapabilityDescriptor> {
        let descriptors = self.descriptors.as_mut()?;
        if descriptors
            .peek(key)
            .is_some_and(|descriptor| descriptor.revision != revision)
        {
            descriptors.pop(key);
            return None;
        }
        match self.eviction {
            EvictionPolicy::Lru => descriptors.get(key).cloned(),
            EvictionPolicy::Fifo => descriptors.peek(key).cloned(),
        }
    }

    pub(super) fn insert(&mut self, descriptor: CapabilityDescriptor, epoch: &Arc<()>) {
        if !Arc::ptr_eq(epoch, &self.epoch) {
            return;
        }
        if let Some(descriptors) = &mut self.descriptors {
            let key = (descriptor.id.clone(), descriptor.visibility.clone());
            descriptors.put(key, descriptor);
        }
    }

    pub(super) fn remove(&mut self, key: &CacheKey) {
        if let Some(descriptors) = &mut self.descriptors {
            descriptors.pop(key);
        }
    }

    pub(super) fn invalidate_id(&mut self, id: &CapabilityId) {
        self.epoch = Arc::new(());
        if let Some(descriptors) = &mut self.descriptors {
            let keys: Vec<_> = descriptors
                .iter()
                .filter(|(key, _)| &key.0 == id)
                .map(|(key, _)| key.clone())
                .collect();
            for key in keys {
                descriptors.pop(&key);
            }
        }
    }

    pub(super) fn clear(&mut self) {
        self.epoch = Arc::new(());
        if let Some(descriptors) = &mut self.descriptors {
            descriptors.clear();
        }
    }
}
