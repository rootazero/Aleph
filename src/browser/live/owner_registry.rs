//! Exact operation-bound owner routing, with compare-and-retire capabilities.
//!
//! Tombstones are bounded diagnostics, NOT authority: a revoked registration
//! stays revoked even after its tombstone is evicted. Registering again requires
//! an internal successful operation record; a wire binding alone cannot register.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use aleph_protocol::browser_live::BrowserTargetBinding;

use super::navigation_gate::{DocumentBoundarySink, OwnerTerminalSink};
use super::LiveProfileOwner;
use crate::browser::cdp_backend::BrowserOperationRecord;
use crate::browser::error::BrowserError;

const TOMBSTONE_LIMIT: usize = 256;
type Key = (String, String);

fn key(binding: &BrowserTargetBinding) -> Key {
    (
        binding.browser_session_id.clone(),
        binding.target_id.clone(),
    )
}

fn denied() -> BrowserError {
    BrowserError::ActionFailed("exact live owner route is retired or unavailable".into())
}

#[derive(Default)]
struct State {
    epoch: u64,
    active: HashMap<Key, Arc<Registration>>,
    tombstones: VecDeque<(Key, u64)>,
    /// Receiver of document boundaries from every owner registered here.
    /// Installed on an owner when it registers an operation, which always
    /// precedes any resource that could be approved on it.
    boundary_sink: Option<Arc<dyn DocumentBoundarySink>>,
    /// Receiver of target-destroyed / owner-ended notifications; installed
    /// exactly like `boundary_sink`.
    terminal_sink: Option<Arc<dyn OwnerTerminalSink>>,
}

/// Strong ownership includes the operation's engine handle. The owner's CDP
/// session resolver is weak to that handle; retaining ONLY the owner is not enough.
struct Registration {
    epoch: u64,
    record: BrowserOperationRecord,
}

#[derive(Clone, Default)]
pub(crate) struct LiveOwnerRegistry(Arc<Mutex<State>>);

/// Internal, non-serializable compare token and prepared input capability.
/// Keeping the registry and registration alive also keeps owner + engine alive.
#[derive(Clone)]
pub(crate) struct OwnerRouteLease {
    registry: LiveOwnerRegistry,
    registration: Arc<Registration>,
}

impl OwnerRouteLease {
    pub(crate) fn binding(&self) -> BrowserTargetBinding {
        self.registration.record.target_binding()
    }

    pub(crate) fn owner(&self) -> &Arc<LiveProfileOwner> {
        self.registration.record.owner()
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.registration.epoch
    }

    pub(crate) fn validate(&self) -> Result<(), BrowserError> {
        let binding = self.binding();
        let st = self
            .registry
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !st.active.get(&key(&binding)).is_some_and(|current| {
            Arc::ptr_eq(current, &self.registration) && current.epoch == self.epoch()
        }) || !self.owner().route_available(&binding.target_id)
        {
            return Err(denied());
        }
        Ok(())
    }
}

impl LiveOwnerRegistry {
    /// Install the document-boundary sink (the lifecycle's) on this registry
    /// and on every owner already registered. Composition calls this once,
    /// before the first operation can register.
    pub(crate) fn set_boundary_sink(&self, sink: Arc<dyn DocumentBoundarySink>) {
        // Lock order: the State lock only updates State and snapshots owners;
        // sinks are added to owners AFTER it is released (never under it).
        let owners = {
            let mut st = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            st.boundary_sink = Some(sink.clone());
            registered_owners(&st)
        };
        for owner in owners {
            owner.add_boundary_sink(sink.clone());
        }
    }

    /// Install the owner terminal sink (the lifecycle's) on this registry and
    /// on every owner already registered; owners registered later get it in
    /// `register_operation_fresh`. The sink only closes resources: it never
    /// retires routes (route retirement stays with the registry's own paths).
    pub(crate) fn set_terminal_sink(&self, sink: Arc<dyn OwnerTerminalSink>) {
        // `add_terminal_sink` replays `owner_ended` synchronously for an
        // already-ended owner, and that replay may take the lifecycle's
        // publication/bus locks. It must therefore run with NO registry lock
        // held. An owner registered concurrently either sees the sink already
        // installed (registration path) or is in this snapshot; both dedupe.
        let owners = {
            let mut st = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            st.terminal_sink = Some(sink.clone());
            registered_owners(&st)
        };
        for owner in owners {
            owner.add_terminal_sink(sink.clone());
        }
    }

    /// The ONLY registration authority is the record captured inside the exact
    /// owner's mutation scope. No profile/current-tab/PID/URL discovery occurs.
    pub(crate) fn register_operation(
        &self,
        record: &BrowserOperationRecord,
    ) -> Result<OwnerRouteLease, BrowserError> {
        self.register_operation_fresh(record)
            .map(|(lease, _)| lease)
    }

    /// As [`Self::register_operation`], also reporting whether THIS call created
    /// the registration (`true`) or joined an existing identical-owner one
    /// (`false`). A publisher that fails afterwards may compensate with
    /// `retire_target` only for a newly created registration, never one that
    /// may already back a live resource.
    pub(crate) fn register_operation_fresh(
        &self,
        record: &BrowserOperationRecord,
    ) -> Result<(OwnerRouteLease, bool), BrowserError> {
        let binding = record.target_binding();
        // The State lock only decides/updates State and snapshots the sinks to
        // install. Installing them runs AFTER it is released: an owner that
        // already ended replays `owner_ended` synchronously from
        // `add_terminal_sink`, which may take publication/bus locks. The sinks
        // are still installed before this returns, i.e. before any publish.
        let (lease, fresh, boundary_sink, terminal_sink) = {
            let mut st = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            if !record.owner().route_available(&binding.target_id) {
                return Err(denied());
            }
            let k = key(&binding);
            let boundary_sink = st.boundary_sink.clone();
            let terminal_sink = st.terminal_sink.clone();
            if let Some(current) = st.active.get(&k) {
                if !Arc::ptr_eq(current.record.owner(), record.owner()) {
                    return Err(denied());
                }
                (
                    OwnerRouteLease {
                        registry: self.clone(),
                        registration: current.clone(),
                    },
                    false,
                    boundary_sink,
                    terminal_sink,
                )
            } else {
                st.epoch = st.epoch.checked_add(1).ok_or_else(denied)?;
                let registration = Arc::new(Registration {
                    epoch: st.epoch,
                    record: record.clone(),
                });
                st.active.insert(k, registration.clone());
                (
                    OwnerRouteLease {
                        registry: self.clone(),
                        registration,
                    },
                    true,
                    boundary_sink,
                    terminal_sink,
                )
            }
        };
        // Before this operation can lead to an approved resource, its owner
        // must report document boundaries and terminal events to the lifecycle.
        // An owner that ended after registration replays from `add_terminal_sink`.
        if let Some(sink) = boundary_sink {
            record.owner().add_boundary_sink(sink);
        }
        if let Some(sink) = terminal_sink {
            record.owner().add_terminal_sink(sink);
        }
        Ok((lease, fresh))
    }

    #[cfg(test)]
    pub(crate) fn active_count(&self) -> usize {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active
            .len()
    }

    /// Lookup is exact session + target, with profile checked for consistency,
    /// never a profile-based fallback. Dispatch MUST retain the resulting lease.
    pub(crate) fn lookup_exact(
        &self,
        binding: &BrowserTargetBinding,
    ) -> Result<OwnerRouteLease, BrowserError> {
        let registration = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active
            .get(&key(binding))
            .cloned()
            .ok_or_else(denied)?;
        let lease = OwnerRouteLease {
            registry: self.clone(),
            registration,
        };
        if lease.binding() != *binding {
            return Err(denied());
        }
        lease.validate()?;
        Ok(lease)
    }

    pub(crate) fn accepts(&self, lease: &OwnerRouteLease) -> Result<(), BrowserError> {
        if !Arc::ptr_eq(&self.0, &lease.registry.0) {
            return Err(denied());
        }
        lease.validate()
    }

    /// A late callback may retire only ITS generation, never its replacement.
    /// A subsequent successful operation may explicitly register the target again.
    pub(crate) fn retire_target(&self, token: &OwnerRouteLease) -> bool {
        if !Arc::ptr_eq(&self.0, &token.registry.0) {
            return false;
        }
        let mut st = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let k = key(&token.binding());
        if !st
            .active
            .get(&k)
            .is_some_and(|r| Arc::ptr_eq(r, &token.registration))
        {
            return false;
        }
        st.active.remove(&k);
        tombstone(&mut st, k, token.epoch());
        true
    }

    /// Permanently retire this owner for routing (no CDP/engine side effects).
    /// Compare first: a replaced target token cannot retire a newer registration.
    pub(crate) fn retire_owner(&self, token: &OwnerRouteLease) -> bool {
        if !Arc::ptr_eq(&self.0, &token.registry.0) {
            return false;
        }
        let mut st = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !st
            .active
            .get(&key(&token.binding()))
            .is_some_and(|r| Arc::ptr_eq(r, &token.registration))
        {
            return false;
        }
        token.owner().retire_routing();
        let removed: Vec<_> = st
            .active
            .iter()
            .filter(|(_, r)| Arc::ptr_eq(r.record.owner(), token.owner()))
            .map(|(k, r)| (k.clone(), r.epoch))
            .collect();
        for (k, epoch) in removed {
            st.active.remove(&k);
            tombstone(&mut st, k, epoch);
        }
        true
    }
}

/// Distinct owners with an active registration, snapshotted so sinks can be
/// installed after the State lock is released.
fn registered_owners(st: &State) -> Vec<Arc<LiveProfileOwner>> {
    let mut owners: Vec<Arc<LiveProfileOwner>> = Vec::new();
    for registration in st.active.values() {
        let owner = registration.record.owner();
        if !owners.iter().any(|known| Arc::ptr_eq(known, owner)) {
            owners.push(owner.clone());
        }
    }
    owners
}

fn tombstone(st: &mut State, key: Key, epoch: u64) {
    st.tombstones.push_back((key, epoch));
    while st.tombstones.len() > TOMBSTONE_LIMIT {
        st.tombstones.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::fixture;
    use super::*;
    use crate::browser::cdp_backend::BrowserOperationKind;
    use crate::browser::engine::EngineHandle;

    pub(super) fn record(
        owner: &Arc<LiveProfileOwner>,
        handle: &Arc<EngineHandle>,
        target: &str,
    ) -> BrowserOperationRecord {
        BrowserOperationRecord::capture_for_test(
            owner,
            handle,
            target.into(),
            BrowserOperationKind::Open,
        )
    }

    #[tokio::test]
    async fn same_profile_and_target_different_sessions_do_not_cross_route() {
        let (_sa, oa, ha) = fixture().await;
        let (_sb, ob, hb) = fixture().await;
        let registry = LiveOwnerRegistry::default();
        let a = registry
            .register_operation(&record(&oa, &ha, "T1"))
            .unwrap();
        let b = registry
            .register_operation(&record(&ob, &hb, "T1"))
            .unwrap();
        assert_eq!(a.binding().profile, b.binding().profile);
        assert_eq!(a.binding().target_id, b.binding().target_id);
        assert_ne!(
            a.binding().browser_session_id,
            b.binding().browser_session_id
        );
        assert!(Arc::ptr_eq(
            registry.lookup_exact(&a.binding()).unwrap().owner(),
            &oa
        ));
        assert!(Arc::ptr_eq(
            registry.lookup_exact(&b.binding()).unwrap().owner(),
            &ob
        ));
        assert!(registry.retire_owner(&a));
        assert!(registry.lookup_exact(&a.binding()).is_err());
        assert!(registry.lookup_exact(&b.binding()).is_ok());
        assert!(!registry.retire_owner(&a));
        assert!(registry
            .register_operation(&record(&oa, &ha, "T1"))
            .is_err());
        b.validate().unwrap();
    }

    #[tokio::test]
    async fn old_registration_and_late_callbacks_fail_closed_after_replacement() {
        let (_s, owner, handle) = fixture().await;
        let registry = LiveOwnerRegistry::default();
        let operation = record(&owner, &handle, "T1");
        let old = registry.register_operation(&operation).unwrap();
        assert!(registry.retire_target(&old));
        let new = registry.register_operation(&operation).unwrap();
        assert!(new.epoch() > old.epoch());
        assert!(old.validate().is_err());
        assert!(!registry.retire_target(&old));
        assert!(!registry.retire_owner(&old));
        assert_eq!(
            registry.lookup_exact(&new.binding()).unwrap().epoch(),
            new.epoch()
        );
        let foreign = LiveOwnerRegistry::default();
        assert!(foreign.accepts(&new).is_err());
        assert!(!foreign.retire_target(&new));
        new.validate().unwrap();
    }

    #[tokio::test]
    async fn strong_owner_and_engine_lifetime_survives_per_call_record_drop() {
        let (_s, owner, handle) = fixture().await;
        let weak_owner = Arc::downgrade(&owner);
        let weak_handle = Arc::downgrade(&handle);
        let registry = LiveOwnerRegistry::default();
        let route = registry
            .register_operation(&record(&owner, &handle, "T1"))
            .unwrap();
        drop(owner);
        drop(handle);
        assert!(weak_owner.upgrade().is_some());
        assert!(weak_handle.upgrade().is_some());
        // Exercises the real weak resolver, not just Arc counts.
        route.owner().attach_target(route.binding()).await.unwrap();
        route.owner().close_target("T1").await.unwrap();
        assert!(registry.retire_target(&route));
        drop(route);
        assert!(weak_handle.upgrade().is_none());
        assert!(weak_owner.upgrade().is_none());
    }

    #[tokio::test]
    async fn tombstones_are_bounded_and_old_tokens_stay_dead_after_eviction() {
        let (_s, owner, handle) = fixture().await;
        let registry = LiveOwnerRegistry::default();
        let old = registry
            .register_operation(&record(&owner, &handle, "T1"))
            .unwrap();
        assert!(registry.retire_target(&old));
        for i in 0..TOMBSTONE_LIMIT + 4 {
            let route = registry
                .register_operation(&record(&owner, &handle, &format!("T{i}")))
                .unwrap();
            assert!(registry.retire_target(&route));
        }
        assert_eq!(registry.0.lock().unwrap().tombstones.len(), TOMBSTONE_LIMIT);
        assert!(old.validate().is_err());
        assert!(registry.lookup_exact(&old.binding()).is_err());
        assert!(!registry.retire_target(&old));
    }

    #[tokio::test]
    async fn destroyed_target_rejects_record_and_lease_without_affecting_other_targets() {
        let (_s, owner, handle) = fixture().await;
        let registry = LiveOwnerRegistry::default();
        let operation = record(&owner, &handle, "T1");
        let dead = registry.register_operation(&operation).unwrap();
        let live = registry
            .register_operation(&record(&owner, &handle, "T2"))
            .unwrap();
        owner.dispatch(&Arc::new(aleph_cdp::CdpEvent {
            session: None,
            method: "Target.targetDestroyed".into(),
            params: serde_json::json!({ "targetId": "T1" }),
        }));
        assert!(registry.register_operation(&operation).is_err());
        assert!(registry.lookup_exact(&dead.binding()).is_err());
        assert!(dead.validate().is_err());
        live.validate().unwrap();
    }

    #[tokio::test]
    async fn ended_owner_rejects_registration_lookup_and_prepared_lease() {
        let (_s, owner, handle) = fixture().await;
        let registry = LiveOwnerRegistry::default();
        let operation = record(&owner, &handle, "T1");
        let route = registry.register_operation(&operation).unwrap();
        owner.retire();
        assert!(registry.register_operation(&operation).is_err());
        assert!(registry.lookup_exact(&route.binding()).is_err());
        assert!(route.validate().is_err());
    }
}
