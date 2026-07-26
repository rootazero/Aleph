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
    /// Monotonic per-push identity. Two toasts with the same text and kind are
    /// still two different toasts — this field is what lets a stale timer tell
    /// "mine is still up" from "an identical-looking newer one replaced it".
    /// Content equality cannot do that job: the real call sites push the same
    /// static string every time (`toast_deleted` on each single-row delete,
    /// `toast_copied` on each copy), so two consecutive toasts are routinely
    /// indistinguishable by content.
    pub seq: u64,
    pub text: String,
    pub kind: ToastKind,
}

/// A single-slot toast holder. A newer message replaces an older one rather
/// than stacking: these are confirmations of the user's own click, so the most
/// recent one is the only one still relevant.
pub type ToastSlot = RwSignal<Option<ToastMsg>>;

/// How long a toast stays up.
const TOAST_MS: u64 = 2_400;

/// Show `text`, clearing it after [`TOAST_MS`].
///
/// The timer is keyed on the message identity: if another toast replaces this
/// one before the timeout fires, the stale timer must not blank the new
/// message, so it checks before clearing.
pub fn push_toast(slot: ToastSlot, text: String, kind: ToastKind) {
    let seq = next_seq();
    slot.set(Some(ToastMsg { seq, text, kind }));
    set_timeout(
        move || {
            if should_clear(slot.get_untracked().map(|m| m.seq), seq) {
                slot.set(None);
            }
        },
        std::time::Duration::from_millis(TOAST_MS),
    );
}

/// Next per-push identity. WASM is single-threaded, so a thread-local counter
/// is a real monotonic sequence here — and unlike a timestamp it cannot collide
/// for two pushes landing in the same tick.
fn next_seq() -> u64 {
    use std::cell::Cell;
    thread_local!(static SEQ: Cell<u64> = const { Cell::new(0) });
    SEQ.with(|s| {
        let n = s.get().wrapping_add(1);
        s.set(n);
        n
    })
}

/// Whether an expiring timer owns the toast currently on screen.
///
/// Split out from the timer closure so the decision is testable without a
/// browser event loop. The test that matters is the one where two pushes carry
/// identical text and kind: their `seq` differ, so the older timer must not
/// clear the newer toast.
fn should_clear(current: Option<u64>, expected: u64) -> bool {
    current == Some(expected)
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

    #[test]
    fn clears_when_the_slot_still_holds_the_same_seq() {
        assert!(should_clear(Some(7), 7));
    }

    #[test]
    fn does_not_clear_when_a_newer_push_replaced_it_even_with_identical_content() {
        // This is the decisive case: two pushes with the *same* text and kind
        // (e.g. two single-row deletes, both pushing `toast_deleted` verbatim)
        // are still two different toasts. A content-equality implementation of
        // `should_clear` would wrongly return `true` here, since `old` and
        // `new` render identically -- only `seq` tells them apart. This test
        // fails against that older, content-equality version.
        let old = ToastMsg {
            seq: next_seq(),
            text: "Deleted".to_string(),
            kind: ToastKind::Success,
        };
        let new = ToastMsg {
            seq: next_seq(),
            text: "Deleted".to_string(),
            kind: ToastKind::Success,
        };
        assert_eq!(old.text, new.text);
        assert_eq!(old.kind, new.kind);
        assert_ne!(old.seq, new.seq);
        assert!(!should_clear(Some(new.seq), old.seq));
    }

    #[test]
    fn does_not_clear_an_already_empty_slot() {
        assert!(!should_clear(None, 1));
    }

    #[test]
    fn next_seq_is_strictly_increasing_across_consecutive_calls() {
        let a = next_seq();
        let b = next_seq();
        let c = next_seq();
        assert!(a < b);
        assert!(b < c);
    }
}
