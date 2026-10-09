//! Pure reconciliation for effect-claim event histories.

use crate::session::events::{ClaimStateWire, OwnerRefWire, SessionEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectClaimState { Prepared, Claimed, Invoking, Succeeded, Failed, Unknown }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectClaimEvent {
    pub request_id: String,
    pub owner: String,
    pub owner_generation: u64,
    pub fence: u64,
    pub state: EffectClaimState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectClaimReconciliation { pub request_id: String, pub terminal: EffectClaimState }

fn owner_name(owner: &OwnerRefWire) -> String {
    match owner {
        OwnerRefWire::Runtime => "runtime".into(),
        OwnerRefWire::Session(id) => format!("session:{id}"),
        OwnerRefWire::Run(id) => format!("run:{id}"),
        OwnerRefWire::Task(id) => format!("task:{id}"),
    }
}

/// Convert durable session receipts into reducer input. Events without a claim
/// receipt are deliberately ignored; the adapter does not replay effects.
pub fn effect_claim_event_from_session(event: &SessionEvent) -> Option<EffectClaimEvent> {
    match event {
        SessionEvent::EffectClaimPrepared { request_id, owner, owner_generation, fence, .. } => Some(EffectClaimEvent {
            request_id: request_id.clone(), owner: owner_name(owner), owner_generation: *owner_generation,
            fence: *fence, state: EffectClaimState::Prepared,
        }),
        SessionEvent::EffectClaimClaimed { request_id, fence, owner, owner_generation, .. } => Some(EffectClaimEvent {
            request_id: request_id.clone(), owner: owner_name(owner), owner_generation: *owner_generation,
            fence: *fence, state: EffectClaimState::Claimed,
        }),
        SessionEvent::EffectClaimInvoking { request_id, fence, owner, owner_generation, .. } => Some(EffectClaimEvent {
            request_id: request_id.clone(), owner: owner_name(owner), owner_generation: *owner_generation,
            fence: *fence, state: EffectClaimState::Invoking,
        }),
        SessionEvent::EffectClaimTerminal { request_id, fence, owner, owner_generation, state, .. } => Some(EffectClaimEvent {
            request_id: request_id.clone(), owner: owner_name(owner), owner_generation: *owner_generation,
            fence: *fence, state: match state {
                ClaimStateWire::Prepared => EffectClaimState::Prepared,
                ClaimStateWire::Active => EffectClaimState::Claimed,
                ClaimStateWire::Invoking => EffectClaimState::Invoking,
                ClaimStateWire::Succeeded => EffectClaimState::Succeeded,
                ClaimStateWire::Failed => EffectClaimState::Failed,
                ClaimStateWire::Unknown => EffectClaimState::Unknown,
            },
        }),
        _ => None,
    }
}

/// Reconcile only the durable receipts. Prepared and invoking are not inferred
/// from adjacent events: without both carriers the answer remains Unknown.
pub fn reconcile_session_events(events: &[SessionEvent]) -> EffectClaimReconciliation {
    let mapped: Vec<_> = events.iter().filter_map(effect_claim_event_from_session).collect();
    reconcile_effect_claim(&mapped)
}

pub fn reconcile_effect_claim(events: &[EffectClaimEvent]) -> EffectClaimReconciliation {
    let request_id = events.first().map_or_else(String::new, |event| event.request_id.clone());
    let unknown = || EffectClaimReconciliation { request_id: request_id.clone(), terminal: EffectClaimState::Unknown };
    let Some(first) = events.first() else { return unknown(); };
    if first.request_id.is_empty() || first.owner.is_empty() || first.owner_generation == 0 || first.fence == 0 || first.state != EffectClaimState::Prepared { return unknown(); }
    let identity = (first.request_id.as_str(), first.owner.as_str(), first.owner_generation, first.fence);
    let mut current = EffectClaimState::Prepared;
    let mut terminal = None;
    for (index, event) in events.iter().enumerate() {
        if event.request_id.is_empty() || event.owner.is_empty() || event.owner_generation == 0 || event.fence == 0
            || (event.request_id.as_str(), event.owner.as_str(), event.owner_generation, event.fence) != identity
            || terminal.is_some() { return unknown(); }
        match (index, event.state) {
            (0, EffectClaimState::Prepared) => {}
            (_, EffectClaimState::Prepared) => return unknown(),
            (_, EffectClaimState::Claimed) if current == EffectClaimState::Prepared => current = EffectClaimState::Claimed,
            (_, EffectClaimState::Invoking) if current == EffectClaimState::Claimed => current = EffectClaimState::Invoking,
            (_, state @ (EffectClaimState::Succeeded | EffectClaimState::Failed)) if current == EffectClaimState::Invoking => { current = state; terminal = Some(state); }
            // spec §7.5: dispose / revoke / unobservable external effect may land in
            // Unknown from any active (non-terminal) state; Unknown is terminal.
            (_, EffectClaimState::Unknown) if matches!(current, EffectClaimState::Prepared | EffectClaimState::Claimed | EffectClaimState::Invoking) => { current = EffectClaimState::Unknown; terminal = Some(EffectClaimState::Unknown); }
            _ => return unknown(),
        }
    }
    EffectClaimReconciliation { request_id, terminal: terminal.unwrap_or(EffectClaimState::Unknown) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{ClaimStateWire, OwnerRefWire, SessionEvent};
    fn event(state: EffectClaimState) -> EffectClaimEvent { EffectClaimEvent { request_id: "r".into(), owner: "o".into(), owner_generation: 1, fence: 1, state } }
    #[test] fn legal_chain() { assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Prepared),event(EffectClaimState::Claimed),event(EffectClaimState::Invoking),event(EffectClaimState::Succeeded)]).terminal, EffectClaimState::Succeeded); }
    #[test] fn no_claimed_invoking() { assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Prepared),event(EffectClaimState::Invoking)]).terminal, EffectClaimState::Unknown); }
    #[test] fn duplicate_active_claim() { assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Prepared),event(EffectClaimState::Claimed),event(EffectClaimState::Claimed)]).terminal, EffectClaimState::Unknown); }
    #[test] fn post_terminal_is_unknown() { assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Prepared),event(EffectClaimState::Claimed),event(EffectClaimState::Invoking),event(EffectClaimState::Failed),event(EffectClaimState::Succeeded)]).terminal, EffectClaimState::Unknown); }
    #[test]
    fn duplicate_claim_and_illegal_migration_remain_fail_closed() {
        // Already-pinned semantics must not regress when producer integration
        // lands later. Duplicate active claim and terminal-then-migrate both
        // close to Unknown — never "ignore", never a second-idempotent result.
        assert_eq!(
            reconcile_effect_claim(&[
                event(EffectClaimState::Prepared),
                event(EffectClaimState::Claimed),
                event(EffectClaimState::Claimed),
            ])
            .terminal,
            EffectClaimState::Unknown
        );
        assert_eq!(
            reconcile_effect_claim(&[
                event(EffectClaimState::Prepared),
                event(EffectClaimState::Claimed),
                event(EffectClaimState::Invoking),
                event(EffectClaimState::Succeeded),
                event(EffectClaimState::Succeeded),
            ])
            .terminal,
            EffectClaimState::Unknown
        );
    }
    #[test] fn identity_mismatch_is_unknown() { let mut changed=event(EffectClaimState::Claimed); changed.owner="other".into(); assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Prepared),changed]).terminal, EffectClaimState::Unknown); }
    #[test] fn missing_memo_is_fail_closed() { assert_eq!(reconcile_effect_claim(&[]).terminal, EffectClaimState::Unknown); assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Unknown)]).terminal, EffectClaimState::Unknown); }
    #[test] fn failed_claim_is_not_replayed() { assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Prepared),event(EffectClaimState::Claimed),event(EffectClaimState::Invoking),event(EffectClaimState::Failed)]).terminal, EffectClaimState::Failed); }

    // spec §7.5 active→Unknown closures (three legal paths).
    #[test] fn prepared_to_unknown() { assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Prepared),event(EffectClaimState::Unknown)]).terminal, EffectClaimState::Unknown); }
    #[test] fn claimed_to_unknown() { assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Prepared),event(EffectClaimState::Claimed),event(EffectClaimState::Unknown)]).terminal, EffectClaimState::Unknown); }
    #[test] fn invoking_to_unknown() { assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Prepared),event(EffectClaimState::Claimed),event(EffectClaimState::Invoking),event(EffectClaimState::Unknown)]).terminal, EffectClaimState::Unknown); }

    // Unknown is terminal: any event after a terminal state fails closed.
    #[test] fn event_after_unknown_is_unknown() { assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Prepared),event(EffectClaimState::Unknown),event(EffectClaimState::Claimed)]).terminal, EffectClaimState::Unknown); }
    #[test] fn duplicate_terminal_is_unknown() { assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Prepared),event(EffectClaimState::Claimed),event(EffectClaimState::Invoking),event(EffectClaimState::Succeeded),event(EffectClaimState::Succeeded)]).terminal, EffectClaimState::Unknown); }
    #[test] fn terminal_after_succeeded_is_unknown() { assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Prepared),event(EffectClaimState::Claimed),event(EffectClaimState::Invoking),event(EffectClaimState::Succeeded),event(EffectClaimState::Unknown)]).terminal, EffectClaimState::Unknown); }

    fn ev(request_id: &str, owner: &str, gen: u64, fence: u64, state: EffectClaimState) -> EffectClaimEvent {
        EffectClaimEvent { request_id: request_id.into(), owner: owner.into(), owner_generation: gen, fence, state }
    }
    #[test] fn empty_or_zero_identity_fail_closed() {
        assert_eq!(reconcile_effect_claim(&[ev("", "o", 1, 1, EffectClaimState::Prepared)]).terminal, EffectClaimState::Unknown);
        assert_eq!(reconcile_effect_claim(&[ev("r", "", 1, 1, EffectClaimState::Prepared)]).terminal, EffectClaimState::Unknown);
        assert_eq!(reconcile_effect_claim(&[ev("r", "o", 0, 1, EffectClaimState::Prepared)]).terminal, EffectClaimState::Unknown);
        assert_eq!(reconcile_effect_claim(&[ev("r", "o", 1, 0, EffectClaimState::Prepared)]).terminal, EffectClaimState::Unknown);
    }
    #[test] fn zero_fence_on_later_event_fail_closed() { assert_eq!(reconcile_effect_claim(&[ev("r","o",1,1,EffectClaimState::Prepared),ev("r","o",1,0,EffectClaimState::Claimed)]).terminal, EffectClaimState::Unknown); }

    // Adapter path: EffectClaimTerminal{state: Unknown} maps to terminal Unknown.
    #[test] fn session_terminal_unknown_maps_to_unknown() {
        let events = vec![
            SessionEvent::EffectClaimPrepared { request_id: "r".into(), owner: OwnerRefWire::Runtime, owner_generation: 1, fence: 1, at: 0 },
            SessionEvent::EffectClaimTerminal { request_id: "r".into(), owner: OwnerRefWire::Runtime, owner_generation: 1, fence: 1, state: ClaimStateWire::Unknown, at: 1 },
        ];
        let rec = reconcile_session_events(&events);
        assert_eq!(rec.request_id, "r");
        assert_eq!(rec.terminal, EffectClaimState::Unknown);
    }
}
