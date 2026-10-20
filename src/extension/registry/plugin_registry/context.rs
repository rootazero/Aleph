//! Context registrations share the extension registry and its lifecycle lock.

use super::PluginRegistry;
use crate::context::fabric::{
    AdmittedContextCall, AuthenticatedCaller, ContextCapabilityDescriptor, ContextCapabilityMode,
    ContextCapabilityRequest, ContextNamespace, ContextOperation,
};
use crate::extension::capability::{ContextGrant, ContextGrantAudit, ContextGrantError};
use crate::extension::registry::{
    namespaced_component_key, ContextRegistration, ContextRegistryError, ContextStats,
};
use std::collections::{HashSet, VecDeque};

const CONTEXT_AUDIT_CAP: usize = 256;

fn final_owner(mode: ContextCapabilityMode) -> bool {
    matches!(
        mode,
        ContextCapabilityMode::Replace | ContextCapabilityMode::Bundle
    )
}

fn record_context_audit(
    records: &mut VecDeque<ContextGrantAudit>,
    total: &mut u64,
    audit: ContextGrantAudit,
) {
    if records.len() >= CONTEXT_AUDIT_CAP {
        records.pop_front();
    }
    records.push_back(audit);
    *total = total.saturating_add(1);
}

impl PluginRegistry {
    fn context_owner_active(&self, registration: &ContextRegistration) -> bool {
        let id = &registration.plugin_id;
        id.is_empty() || self.plugins.get(id).is_some_and(|p| p.status.is_active())
    }

    fn check_context_registration(
        &self,
        registration: &ContextRegistration,
        grant: &ContextGrant,
        now: u64,
    ) -> Result<(), ContextRegistryError> {
        let descriptor = &registration.descriptor;
        grant.validate_against(descriptor, now)?;
        let builtin = grant.plugin_id.is_empty();
        if registration.plugin_id != grant.plugin_id
            || registration.owner_mode != descriptor.mode
            || registration.bundle_id != descriptor.bundle_id
            || builtin != (registration.owner_mode == ContextCapabilityMode::Builtin)
            || (builtin && registration.lifecycle_handle.is_some())
            || (!builtin && registration.lifecycle_handle.as_deref() != Some(&grant.plugin_id))
        {
            return Err(ContextRegistryError::InvalidOwnership);
        }
        if !self.context_owner_active(registration) {
            return Err(ContextRegistryError::PluginUnavailable(
                grant.plugin_id.clone(),
            ));
        }
        if descriptor.permissions.network_egress && !grant.egress {
            return Err(ContextGrantError::EgressDenied.into());
        }
        if descriptor.permissions.project_write && !grant.project_write {
            return Err(ContextGrantError::ProjectWriteDenied.into());
        }
        if descriptor.permissions.cross_session_read && !grant.cross_session {
            return Err(ContextGrantError::CrossSessionDenied.into());
        }
        Ok(())
    }

    pub fn register_context(
        &mut self,
        registration: ContextRegistration,
        now: u64,
    ) -> Result<(), ContextRegistryError> {
        self.register_context_bundle(vec![registration], now)
    }

    pub fn register_context_bundle(
        &mut self,
        registrations: Vec<ContextRegistration>,
        now: u64,
    ) -> Result<(), ContextRegistryError> {
        let result = self.check_context_bundle(&registrations, now);
        for registration in &registrations {
            let grant = registration.supplied_grant();
            record_context_audit(
                &mut self.context_audit,
                &mut self.context_audit_total,
                ContextGrantAudit {
                    component_key: registration.key(),
                    session_id: grant.map(|g| g.session_id.clone()),
                    allowed: result.is_ok(),
                    action: "register".into(),
                    reason: result
                        .as_ref()
                        .err()
                        .map_or_else(|| "registered".into(), ToString::to_string),
                    audit_reason: grant.map(|g| g.audit_reason.clone()),
                    at: now,
                },
            );
        }
        result?;
        for incoming in registrations {
            let key = incoming.key();
            if let Some(existing) = self.contexts.get_mut(&key) {
                let grant = incoming
                    .supplied_grant()
                    .ok_or(ContextRegistryError::InvalidOwnership)?;
                existing.insert_grant(grant.clone());
            } else {
                self.contexts.insert(key, incoming);
            }
        }
        Ok(())
    }

    fn check_context_bundle(
        &self,
        registrations: &[ContextRegistration],
        now: u64,
    ) -> Result<(), ContextRegistryError> {
        let first = registrations
            .first()
            .ok_or(ContextRegistryError::EmptyBundle)?;
        let first_grant = first
            .supplied_grant()
            .ok_or(ContextRegistryError::InvalidOwnership)?;
        let owner = &first.plugin_id;
        let bundle = first.bundle_id.as_deref();
        if registrations.len() > 1 && bundle.is_none() {
            return Err(ContextRegistryError::InvalidBundle);
        }
        if bundle.is_some_and(|id| id.trim().is_empty() || id.trim() != id)
            || registrations.iter().any(|r| {
                r.plugin_id != *owner
                    || r.bundle_id.as_deref() != bundle
                    || r.supplied_grant()
                        .is_none_or(|g| g.session_id != first_grant.session_id)
            })
        {
            return Err(ContextRegistryError::InvalidBundle);
        }
        let mut keys = HashSet::new();
        for (index, registration) in registrations.iter().enumerate() {
            let grant = registration
                .supplied_grant()
                .ok_or(ContextRegistryError::InvalidOwnership)?;
            self.check_context_registration(registration, grant, now)?;
            let key = registration.key();
            if !keys.insert(key.clone()) {
                return Err(ContextRegistryError::DuplicateKey(key));
            }
            if let Some(existing) = self.contexts.get(&key) {
                if existing.descriptor != registration.descriptor
                    || existing.owner_mode != registration.owner_mode
                    || existing.bundle_id != registration.bundle_id
                    || existing.lifecycle_handle != registration.lifecycle_handle
                {
                    return Err(ContextRegistryError::DescriptorMismatch(key));
                }
                if existing
                    .grant_for_session(&grant.session_id)
                    .is_some_and(|old| old.expires_at > now)
                {
                    return Err(ContextRegistryError::DuplicateKey(key));
                }
            }
            for other in self.contexts.values().chain(registrations[..index].iter()) {
                // Ownership is routed by the invoking session. Independent
                // sessions may install the same phase without blocking each other.
                let same_session = other
                    .grant_for_session(&grant.session_id)
                    .is_some_and(|existing_grant| existing_grant.expires_at > now);
                let conflict = same_session
                    && ((final_owner(registration.owner_mode) && final_owner(other.owner_mode))
                        || (registration.owner_mode == ContextCapabilityMode::Builtin
                            && other.owner_mode == ContextCapabilityMode::Builtin));
                if conflict {
                    if let Some(phase) = registration
                        .descriptor
                        .phases
                        .iter()
                        .find(|p| other.descriptor.declares(**p))
                    {
                        return Err(ContextRegistryError::OwnerConflict {
                            phase: *phase,
                            existing: other.key(),
                            incoming: key,
                        });
                    }
                }
            }
        }
        if let Some(bundle_id) = bundle {
            let existing: HashSet<_> = self
                .contexts
                .values()
                .filter(|r| r.plugin_id == *owner && r.bundle_id.as_deref() == Some(bundle_id))
                .map(ContextRegistration::key)
                .collect();
            if !existing.is_empty() && existing != keys {
                return Err(ContextRegistryError::InvalidBundle);
            }
        }
        Ok(())
    }

    pub fn unregister_context_bundle(
        &mut self,
        plugin_id: &str,
        bundle_id: &str,
    ) -> Vec<ContextRegistration> {
        let keys: Vec<_> = self
            .contexts
            .iter()
            .filter(|(_, r)| r.plugin_id == plugin_id && r.bundle_id.as_deref() == Some(bundle_id))
            .map(|(key, _)| key.clone())
            .collect();
        keys.into_iter()
            .filter_map(|key| self.contexts.remove(&key))
            .collect()
    }

    /// Revoke one session's grants while retaining the shared descriptor and
    /// grants belonging to other sessions.
    pub fn unregister_context_bundle_for_session(
        &mut self,
        plugin_id: &str,
        bundle_id: &str,
        session_id: &str,
    ) -> usize {
        let keys: Vec<_> = self
            .contexts
            .iter()
            .filter(|(_, r)| {
                r.plugin_id == plugin_id
                    && r.bundle_id.as_deref() == Some(bundle_id)
                    && r.grant_for_session(session_id).is_some()
            })
            .map(|(key, _)| key.clone())
            .collect();
        let mut removed = 0;
        for key in keys {
            if let Some(registration) = self.contexts.get_mut(&key) {
                removed += usize::from(registration.remove_session(session_id).is_some());
            }
            if self.contexts.get(&key).is_some_and(|r| r.grants.is_empty()) {
                self.contexts.remove(&key);
            }
        }
        removed
    }

    /// Revoke every context grant belonging to one plugin/session.
    pub fn revoke_context_grants(&mut self, plugin_id: &str, session_id: &str) -> usize {
        let keys: Vec<_> = self.contexts.keys().cloned().collect();
        let mut removed = 0;
        for key in keys {
            if let Some(registration) = self.contexts.get_mut(&key) {
                if registration.plugin_id == plugin_id {
                    removed += usize::from(registration.remove_session(session_id).is_some());
                }
            }
            if self.contexts.get(&key).is_some_and(|r| r.grants.is_empty()) {
                self.contexts.remove(&key);
            }
        }
        removed
    }

    #[must_use]
    pub fn resolve_context(&self, key: &str) -> Option<&ContextRegistration> {
        self.contexts
            .get(key)
            .filter(|r| self.context_owner_active(r))
    }

    fn check_context_session(
        &self,
        registration: &ContextRegistration,
        session_id: &str,
        now: u64,
    ) -> Result<(), ContextRegistryError> {
        let grant = registration
            .grant_for_session(session_id)
            .ok_or(ContextGrantError::SessionDenied)?;
        self.check_context_registration(registration, grant, now)?;
        if let Some(bundle_id) = &registration.bundle_id {
            for member in self.contexts.values().filter(|member| {
                member.plugin_id == registration.plugin_id
                    && member.bundle_id.as_ref() == Some(bundle_id)
            }) {
                let grant = member
                    .grant_for_session(session_id)
                    .ok_or(ContextGrantError::SessionDenied)?;
                self.check_context_registration(member, grant, now)?;
            }
        }
        Ok(())
    }

    /// Stable session-scoped query seam for policy derivation.
    #[must_use]
    pub fn eligible_context_keys(&self, session_id: &str, now: u64) -> Vec<String> {
        let mut keys: Vec<_> = self
            .contexts
            .iter()
            .filter_map(|(key, registration)| {
                self.check_context_session(registration, session_id, now)
                    .ok()
                    .map(|_| key.clone())
            })
            .collect();
        keys.sort();
        keys
    }

    #[must_use]
    pub fn list_context_descriptors(&self) -> Vec<&ContextCapabilityDescriptor> {
        let mut registrations: Vec<_> = self
            .contexts
            .iter()
            .filter(|(_, r)| self.context_owner_active(r))
            .collect();
        registrations.sort_by_key(|(key, _)| *key);
        registrations
            .into_iter()
            .map(|(_, r)| &r.descriptor)
            .collect()
    }

    pub fn validate_context_grant<'a>(
        &'a mut self,
        key: &str,
        request: &'a ContextCapabilityRequest,
        caller: &'a AuthenticatedCaller,
        now: u64,
    ) -> Result<AdmittedContextCall<'a>, ContextRegistryError> {
        // Finish whole-registry checks before borrowing the descriptor for the
        // returned call, so the independent audit fields remain mutable.
        let session_check = self
            .contexts
            .get(key)
            .ok_or_else(|| ContextRegistryError::NotFound(key.into()))
            .and_then(|registration| {
                self.check_context_session(registration, caller.session_id(), now)
            });
        let result = (|| {
            session_check?;
            let registration = self
                .contexts
                .get(key)
                .ok_or_else(|| ContextRegistryError::NotFound(key.into()))?;
            let grant = registration
                .grant_for_session(caller.session_id())
                .ok_or(ContextGrantError::SessionDenied)?;
            // Metadata-only notifications carry a snapshot, but their output
            // is discarded. Snapshot presence does not imply a transform.
            if registration.owner_mode == ContextCapabilityMode::Observer
                && request.operation.requires_snapshot()
                && request.operation != ContextOperation::Notify
            {
                return Err(ContextRegistryError::ObserverTransformDenied);
            }
            if let Some(snapshot) = &request.snapshot {
                if snapshot.session_id != caller.session_id()
                    && (!grant.cross_session
                        || grant.namespace != ContextNamespace::CrossSession
                        || !grant.scope.handles.contains(&snapshot.session_id))
                {
                    return Err(ContextGrantError::CrossSessionDenied.into());
                }
                if snapshot.scope.namespace.reach() > grant.namespace.reach()
                    || (snapshot.scope.namespace != ContextNamespace::Session
                        && snapshot.scope.namespace != grant.namespace)
                {
                    return Err(ContextGrantError::ScopeEscalation.into());
                }
                if snapshot.scope.namespace == ContextNamespace::Session
                    && snapshot
                        .scope
                        .handles
                        .iter()
                        .any(|h| h != &snapshot.session_id)
                {
                    return Err(ContextGrantError::CrossSessionDenied.into());
                }
                for handle in &snapshot.scope.handles {
                    let own = snapshot.scope.namespace == ContextNamespace::Session
                        && handle == caller.session_id();
                    if !own && !grant.scope.handles.contains(handle) {
                        return Err(ContextGrantError::ScopeEscalation.into());
                    }
                }
                if snapshot.scope.namespace != ContextNamespace::Session
                    && snapshot.scope.handles.is_empty()
                {
                    return Err(ContextGrantError::ScopeEscalation.into());
                }
            }
            Ok(request.admit(&registration.descriptor, caller)?)
        })();
        let audit_reason = self
            .contexts
            .get(key)
            .and_then(|r| r.grant_for_session(caller.session_id()))
            .map(|g| g.audit_reason.clone());
        record_context_audit(
            &mut self.context_audit,
            &mut self.context_audit_total,
            ContextGrantAudit {
                component_key: key.into(),
                session_id: Some(caller.session_id().into()),
                allowed: result.is_ok(),
                action: "invoke".into(),
                reason: result
                    .as_ref()
                    .err()
                    .map_or_else(|| "admitted".into(), ToString::to_string),
                audit_reason,
                at: now,
            },
        );
        result
    }

    #[must_use]
    pub fn context_stats(&self) -> ContextStats {
        let builtin_registrations = self
            .contexts
            .values()
            .filter(|r| r.plugin_id.is_empty())
            .count();
        ContextStats {
            registrations: self.contexts.len(),
            builtin_registrations,
            plugin_registrations: self.contexts.len() - builtin_registrations,
            bundles: self
                .contexts
                .values()
                .filter_map(|r| {
                    r.bundle_id
                        .as_deref()
                        .map(|id| namespaced_component_key(&r.plugin_id, id))
                })
                .collect::<HashSet<_>>()
                .len(),
            active_plugins: self.list_active_plugins().len(),
            audit_total_events: self.context_audit_total,
            audit_retained_events: self.context_audit.len(),
        }
    }

    #[must_use]
    pub fn context_audit(&self) -> Vec<ContextGrantAudit> {
        self.context_audit.iter().cloned().collect()
    }
}

#[cfg(test)]
#[path = "context_tests.rs"]
mod tests;
