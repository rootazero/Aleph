//! Pure reconciliation for effect-claim event histories.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectClaimState {
    Prepared,
    Claimed,
    Invoking,
    Succeeded,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectClaimEvent {
    pub request_id: String,
    pub owner: String,
    pub owner_generation: u64,
    pub fence: u64,
    pub state: EffectClaimState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectClaimReconciliation {
    pub request_id: String,
    pub terminal: EffectClaimState,
}

pub fn reconcile_effect_claim(events: &[EffectClaimEvent]) -> EffectClaimReconciliation {
    let request_id = events.first().map_or_else(String::new, |event| event.request_id.clone());
    let unknown = || EffectClaimReconciliation {
        request_id: request_id.clone(),
        terminal: EffectClaimState::Unknown,
    };

    let Some(first) = events.first() else {
        return unknown();
    };
    if first.request_id.is_empty()
        || first.owner.is_empty()
        || first.owner_generation == 0
        || first.fence == 0
        || first.state != EffectClaimState::Prepared
    {
        return unknown();
    }

    let identity = (&first.request_id, &first.owner, first.owner_generation, first.fence);
    let mut current = EffectClaimState::Prepared;
    let mut terminal = None;
    for event in events {
        if event.request_id.is_empty()
            || event.owner.is_empty()
            || event.owner_generation == 0
            || event.fence == 0
            || (&event.request_id, &event.owner, event.owner_generation, event.fence) != identity
        {
            return unknown();
        }
        if terminal.is_some() {
            return unknown();
        }
        if event.state == EffectClaimState::Unknown {
            return unknown();
        }
        if event.state == EffectClaimState::Prepared {
            if current != EffectClaimState::Prepared || event.state == current && event as *const _ != first as *const _ {
                return unknown();
            }
        } else if event.state == EffectClaimState::Claimed {
            if current != EffectClaimState::Prepared {
                return unknown();
            }
            current = event.state;
        } else if event.state == EffectClaimState::Invoking {
            if current != EffectClaimState::Claimed {
                return unknown();
            }
            current = event.state;
        } else if matches!(event.state, EffectClaimState::Succeeded | EffectClaimState::Failed) {
            if current != EffectClaimState::Invoking {
                return unknown();
            }
            current = event.state;
            terminal = Some(event.state);
        } else {
            return unknown();
        }
    }

    EffectClaimReconciliation { request_id, terminal: terminal.unwrap_or(EffectClaimState::Unknown) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(state: EffectClaimState) -> EffectClaimEvent {
        EffectClaimEvent { request_id: "r".into(), owner: "o".into(), owner_generation: 1, fence: 1, state }
    }

    #[test]
    fn legal_chain() {
        let events = [event(EffectClaimState::Prepared), event(EffectClaimState::Claimed), event(EffectClaimState::Invoking), event(EffectClaimState::Succeeded)];
        assert_eq!(reconcile_effect_claim(&events).terminal, EffectClaimState::Succeeded);
    }

    #[test]
    fn no_claimed_invoking() {
        assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Prepared), event(EffectClaimState::Invoking)]).terminal, EffectClaimState::Unknown);
    }

    #[test]
    fn duplicate_active_claim() {
        assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Prepared), event(EffectClaimState::Claimed), event(EffectClaimState::Claimed)]).terminal, EffectClaimState::Unknown);
    }

    #[test]
    fn post_terminal_is_unknown() {
        assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Prepared), event(EffectClaimState::Claimed), event(EffectClaimState::Invoking), event(EffectClaimState::Failed), event(EffectClaimState::Succeeded)]).terminal, EffectClaimState::Unknown);
    }

    #[test]
    fn identity_mismatch_is_unknown() {
        let mut changed = event(EffectClaimState::Claimed);
        changed.owner = "other".into();
        assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Prepared), changed]).terminal, EffectClaimState::Unknown);
    }

    #[test]
    fn missing_memo_is_fail_closed() {
        assert_eq!(reconcile_effect_claim(&[]).terminal, EffectClaimState::Unknown);
        assert_eq!(reconcile_effect_claim(&[event(EffectClaimState::Unknown)]).terminal, EffectClaimState::Unknown);
    }

    #[test]
    fn failed_claim_is_not_replayed() {
        let events = [event(EffectClaimState::Prepared), event(EffectClaimState::Claimed), event(EffectClaimState::Invoking), event(EffectClaimState::Failed)];
        assert_eq!(reconcile_effect_claim(&events).terminal, EffectClaimState::Failed);
    }
}
