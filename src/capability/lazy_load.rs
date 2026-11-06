//! Demand loading through Zahir; canonical revisions, never audit delivery,
//! establish whether retained metadata is reusable.

use crate::capability::cache_policy::ResolverCache;
use crate::capability::descriptor::CapabilityDescriptor;
use crate::capability::facade::{Reference, Zahir};
use std::sync::Mutex;

pub(super) fn lookup(
    zahir: &dyn Zahir,
    cache: &Mutex<ResolverCache>,
    reference: &Reference,
) -> Option<CapabilityDescriptor> {
    let key = (reference.id.clone(), reference.visibility.clone());
    // Capture the fence BEFORE entering source code: source callbacks may
    // invalidate this resolver while a revision probe/load is in progress.
    let epoch = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .epoch();
    let revision = zahir.descriptor_revision(reference);
    {
        let mut cache = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(revision) = revision {
            if let Some(descriptor) = cache.get(&key, revision) {
                return Some(descriptor);
            }
        } else {
            // No revision guarantee (or no current visibility): do not reuse
            // metadata, even if a prior resolve populated this key.
            cache.remove(&key);
        }
    }
    let descriptor = zahir.lookup_descriptor(reference)?;
    // Only sources with a stable revision contract benefit from retention.
    if revision.is_some() {
        cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(descriptor.clone(), &epoch);
    }
    Some(descriptor)
}
