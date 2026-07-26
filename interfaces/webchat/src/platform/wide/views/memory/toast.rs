//! Transient action feedback for the memory console.
//!
//! Module-private on purpose. The two other things in this panel called
//! "toast" (`settings/channels/config_template.rs`,
//! `components/extensions/install_flow.rs`) are inline banners with a
//! different shape and lifetime; abstracting across them now would be
//! speculative. Lift this into `components/ui/` when a second real consumer
//! shows up, not before.

use leptos::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToastMsg {
    pub text: String,
    pub kind: ToastKind,
}

/// A single-slot toast holder. A newer message replaces an older one rather
/// than stacking: these are confirmations of the user's own click, so the most
/// recent one is the only one still relevant.
pub type ToastSlot = RwSignal<Option<ToastMsg>>;

/// How long a toast stays up.
const TOAST_MS: u64 = 2_400;

/// Whether an expiring timer for `expected` should blank the slot, given what
/// is currently sitting in it. Pulled out of [`push_toast`] as a pure
/// function so the identity check itself — the part that is easy to get
/// wrong — is unit-testable without a browser event loop.
fn should_clear(current: Option<&ToastMsg>, expected: &ToastMsg) -> bool {
    current == Some(expected)
}

/// Show `text`, clearing it after [`TOAST_MS`].
///
/// The timer is keyed on the message identity: if another toast replaces this
/// one before the timeout fires, the stale timer must not blank the new
/// message, so it checks before clearing.
pub fn push_toast(slot: ToastSlot, text: String, kind: ToastKind) {
    let msg = ToastMsg { text, kind };
    slot.set(Some(msg.clone()));
    set_timeout(
        move || {
            if should_clear(slot.get_untracked().as_ref(), &msg) {
                slot.set(None);
            }
        },
        std::time::Duration::from_millis(TOAST_MS),
    );
}

#[component]
#[must_use]
pub fn ToastHost(slot: ToastSlot) -> impl IntoView {
    view! {
        {move || slot.get().map(|m| {
            let tone = match m.kind {
                ToastKind::Success => "bg-success-subtle text-success border-success/30",
                ToastKind::Error => "bg-danger-subtle text-danger border-danger/30",
            };
            view! {
                <div
                    class=format!(
                        "fixed bottom-6 left-1/2 -translate-x-1/2 z-50 animate-pop-in \
                         rounded-lg border px-4 py-2 text-sm shadow-lg {tone}"
                    )
                    role="status"
                    aria-live="polite"
                >
                    {m.text}
                </div>
            }
        })}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(text: &str) -> ToastMsg {
        ToastMsg {
            text: text.to_string(),
            kind: ToastKind::Success,
        }
    }

    #[test]
    fn clears_when_the_slot_still_holds_the_same_message() {
        let m = msg("saved");
        assert!(should_clear(Some(&m), &m));
    }

    #[test]
    fn does_not_clear_when_a_newer_message_replaced_it() {
        let old = msg("saved");
        let new = msg("deleted");
        assert!(!should_clear(Some(&new), &old));
    }

    #[test]
    fn does_not_clear_an_already_empty_slot() {
        let m = msg("saved");
        assert!(!should_clear(None, &m));
    }
}
