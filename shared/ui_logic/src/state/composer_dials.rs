//! The per-session dials a conversation carries, and what a composer send
//! carries of them.
//!
//! Five knobs are stored on a session (`SessionIdentityMeta.custom`) and
//! resolved by the run loop every turn: the execution tier, the usage mode,
//! the reasoning depth, the memory mode, and the model pin. A client's job is
//! to show what the server is actually enforcing and to let the user change it
//! — and the two ways to get that wrong are both silent.
//!
//! Lives here because every surface has to agree: two composers (wide and
//! phone) build sends from it, three session pickers restore it, and none of
//! them can be host-tested through Leptos.

/// The dials a conversation carries, as one value.
///
/// # Why one struct and not five loose `Option<String>`s
///
/// Because every site that touches them has to touch *all* of them, and the
/// ones that forgot are the bugs this type exists to prevent: the phone's
/// session picker restored the project folder and silently dropped the tier and
/// the mode, so its pills said "follow global" while the run loop kept
/// enforcing what was stored — for the tier, that is the pill which says which
/// tool calls stop for approval.
///
/// A struct literal is exhaustive, so a sixth dial fails to compile at every
/// construction site instead of being applied on one surface and forgotten on
/// the other three. Never build one with `..Default::default()`.
///
/// `None` on a dial means **follow the install default** — never "off". For
/// `think_level` it means something slightly different and worth stating: core
/// resolves depth as request > session > *no directive at all*, so `None`
/// leaves the provider on its own default rather than following a configured
/// global. There is no global reasoning depth for a surface to name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionKnobs {
    /// `custom["exec_tier"]` — which tool calls stop for approval.
    pub exec_tier: Option<String>,
    /// `custom["session_mode"]` — which tools the turn is shown.
    pub mode: Option<String>,
    /// `custom["think_level"]` — how hard the model is asked to reason.
    pub think_level: Option<String>,
    /// `custom["memory_mode"]` — whether memory envelopes are injected.
    pub memory_mode: Option<String>,
    /// `custom["model_pin"]` — the model `select_model` pinned, if any.
    ///
    /// Read-only on a client: the authoritative writer is the tool, which
    /// updates the process-local table the run loop reads *and* the session
    /// row. A pill that patched only the row would take effect after a restart
    /// and not before one, which is why `sessions.patch` refuses this key
    /// outright (`NOT_PATCHABLE`). It rides in this struct anyway because it is
    /// per-session state a surface must restore and must not leak across a
    /// conversation switch — the same contract as its four siblings.
    pub model_pin: Option<String>,
}

/// What one send puts on the wire, per dial. Every field maps to a `chat.send`
/// parameter of the same meaning (`thinking` and `memory` are the wire names).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SendDials {
    pub exec_tier: Option<String>,
    pub mode: Option<String>,
    pub thinking: Option<String>,
    pub memory: Option<String>,
}

/// The dials a send should carry, given the pills' current values and whether
/// the conversation already has a session row (`session_key.is_some()`).
///
/// Two rules, and getting either wrong fails silently in opposite directions:
///
/// * The **tier** rides every send. Drop it and the very first turn of a new
///   conversation runs under the global tier, not the one the user just armed —
///   there is no session row yet to have been patched, so the pill's value can
///   only reach that run by riding the message. It keeps riding afterwards
///   because it is re-armed per send and a stored value does not out-rank a
///   human arming it now.
/// * **Mode, depth and memory** ride only the FIRST send. Once a session row
///   exists it is authoritative, and re-sending a pill's cached value would
///   silently revert a change the model made mid-conversation through its own
///   tools (`session_set_mode`, `self_config`) — the user would see the pill it
///   came from and never learn the setting had moved.
///
/// `model_pin` is deliberately not here: a pin is written by `select_model`,
/// which updates the table the run loop reads directly, and a per-turn model
/// choice rides `model_override` instead.
#[must_use]
pub fn session_dials_for_send(session_exists: bool, pills: &SessionKnobs) -> SendDials {
    let once = |v: &Option<String>| {
        if session_exists {
            None
        } else {
            v.clone()
        }
    };
    SendDials {
        exec_tier: pills.exec_tier.clone(),
        mode: once(&pills.mode),
        thinking: once(&pills.think_level),
        memory: once(&pills.memory_mode),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pills(tier: &str, mode: &str, think: &str, memory: &str) -> SessionKnobs {
        SessionKnobs {
            exec_tier: Some(tier.to_string()),
            mode: Some(mode.to_string()),
            think_level: Some(think.to_string()),
            memory_mode: Some(memory.to_string()),
            model_pin: Some("claude-opus-5".to_string()),
        }
    }

    #[test]
    fn first_send_carries_every_dial() {
        // No session row yet: this is the only chance the pills have to govern
        // the turn they were armed for.
        let d = session_dials_for_send(false, &pills("full", "code", "high", "off"));
        assert_eq!(d.exec_tier.as_deref(), Some("full"));
        assert_eq!(d.mode.as_deref(), Some("code"));
        assert_eq!(d.thinking.as_deref(), Some("high"));
        assert_eq!(d.memory.as_deref(), Some("off"));
    }

    #[test]
    fn a_live_session_still_carries_the_tier() {
        // The tier is re-armed per send; a session row does not out-rank it.
        let d = session_dials_for_send(true, &pills("ask", "code", "high", "off"));
        assert_eq!(d.exec_tier.as_deref(), Some("ask"));
    }

    #[test]
    fn a_live_session_drops_the_store_owned_dials() {
        // The store is authoritative once it exists — re-sending the pills'
        // cached values would revert a mid-conversation `session_set_mode` or
        // `self_config`.
        let d = session_dials_for_send(true, &pills("ask", "chat", "low", "on"));
        assert_eq!(d.mode, None);
        assert_eq!(d.thinking, None);
        assert_eq!(d.memory, None);
    }

    #[test]
    fn follow_global_carries_nothing() {
        // Every pill on "follow global" = no override to carry, on either side
        // of the session boundary.
        let empty = SessionKnobs::default();
        assert_eq!(session_dials_for_send(false, &empty), SendDials::default());
        assert_eq!(session_dials_for_send(true, &empty), SendDials::default());
    }

    /// The pin is session state, not send state. Carrying it would be a second
    /// writer for a value whose only authoritative writer is `select_model`.
    #[test]
    fn the_model_pin_never_rides_a_send() {
        let d = session_dials_for_send(false, &pills("ask", "chat", "low", "on"));
        // `SendDials` structurally has nowhere to put it — this asserts the
        // four fields it does have are the four that mean something on the
        // wire, so a future field cannot be added here without a wire name.
        assert_eq!(
            d,
            SendDials {
                exec_tier: Some("ask".into()),
                mode: Some("chat".into()),
                thinking: Some("low".into()),
                memory: Some("on".into()),
            }
        );
    }
}
