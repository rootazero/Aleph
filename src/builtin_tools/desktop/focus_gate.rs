//! Pre-flight focus check for `type_text` — the one blind input path.
//!
//! Every other way this tool changes the desktop is *addressed*: a click has
//! coordinates, `set_value` / `ax_action` carry a locator. `type_text`
//! synthesizes global key events, which land wherever the OS believes focus is
//! — not necessarily where the model believes it is. If an app stole focus, a
//! click missed, or a modal opened, the characters go into the user's Slack or
//! a terminal, and the old code still answered `{"typed": true}`.
//!
//! [`super::observe`] already asks the AX layer what holds focus, but only
//! *after* the action. This asks the same question *before* it.
//!
//! Two properties this gate must have, and keeps:
//!
//! * **Mechanical.** It reads AX affordances (`secure`, `settable`, `role`) and
//!   pure role names. It never inspects the text being typed, never scores
//!   intent, never classifies the target semantically (R7/P8).
//! * **Fail-open.** Unknown is not a veto. No AX layer at all, an AX query
//!   that errored (permission revoked, helper restarting), or an element whose
//!   `settable` the helper did not report all mean *allow*: Electron, canvas
//!   and terminal apps legitimately focus an `AXWebArea`/`AXGroup` that reports
//!   nothing, and a gate that closed on silence would be the harness overruling
//!   the model. `force: true` is the model's explicit override for the
//!   refusals that remain.
//!
//! The one exception to `force` is a secure text field: that refusal is a hard
//! block. `safety::blocked_app_reason` only blocks credential *apps* (1Password
//! et al) — it cannot see a password box inside Safari or a `sudo` prompt in
//! Terminal, and this closes that hole.

use aleph_protocol::desktop_bridge::methods::ax::AxElement;

use crate::sync_primitives::Arc;

use super::interactable::is_secure;
use super::types::DesktopOutput;

/// Roles that take typed characters even when the AX layer says their value is
/// not settable — a `settable: false` text field is often merely one whose
/// `AXValue` cannot be *written wholesale* while it still accepts keystrokes.
///
/// `AXSecureTextField` is deliberately absent: it never reaches this list —
/// the secure check at the top of [`evaluate`] refuses it first.
const EDITABLE_ROLES: &[&str] = &["AXTextField", "AXTextArea", "AXSearchField", "AXComboBox"];

/// Outcome of the pre-flight check.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Gate {
    Allow,
    Refuse(String),
}

/// Decide whether synthetic keystrokes may be emitted, given what (if anything)
/// currently holds keyboard focus.
///
/// `focused == None` means the AX layer answered "nothing is focused" — not
/// "AX is unavailable"; the caller resolves that distinction before calling in.
pub(super) fn evaluate(focused: Option<&AxElement>, force: bool) -> Gate {
    // Checked before `force`: this is a hard block, not a default the model may
    // argue with. A synthetic keystroke into a password box is how a credential
    // ends up in the wrong place.
    if let Some(el) = focused {
        if is_secure(el) {
            return Gate::Refuse(
                "type_text refused: keyboard focus is on a secure text field (a password box). \
                 Aleph never types into credential fields — ask the user to enter it themselves. \
                 This refusal is not overridable with force."
                    .to_string(),
            );
        }
    }

    if force {
        return Gate::Allow;
    }

    let Some(el) = focused else {
        return Gate::Refuse(
            "type_text refused: nothing holds keyboard focus, so the keystrokes would land in \
             whichever app the system focuses instead — possibly one the user is working in. \
             Click the text input first (desktop_ax_snapshot gives you its center), or use the \
             desktop set_value action, which addresses the element directly and verifies the \
             write. Pass force:true to type anyway."
                .to_string(),
        );
    };

    // `settable: Some(false)` on a non-editable role (a focused button, a
    // static label) is the AX layer stating the target takes no value. Unknown
    // (`None`) never reaches this branch — that is the fail-open case.
    if el.settable == Some(false) && !EDITABLE_ROLES.contains(&el.role.as_str()) {
        let role = &el.role;
        return Gate::Refuse(format!(
            "type_text refused: keyboard focus is on a {role}, which reports that it takes no \
             typed value — the keystrokes would be swallowed or trigger stray shortcuts. Click \
             the input you meant first, or use the desktop set_value action with role / \
             element_title. Pass force:true to type anyway."
        ));
    }

    Gate::Allow
}

/// Run the gate against the live desktop. Returns `Some(refusal)` when the
/// keystrokes must not be emitted, `None` to proceed.
pub(super) async fn check(
    platform: &Arc<dyn aleph_desktop::DesktopPlatform>,
    force: bool,
) -> Option<DesktopOutput> {
    // No accessibility layer at all: a desktop with accessibility switched off, or a
    // headless build. type_text is the *only* input path there — a gate that cannot see
    // must not block.
    let ax = platform.ax()?;

    let focused = match ax.query_focused().await {
        Ok(el) => el,
        Err(e) => {
            // AX is momentarily unreachable (permission revoked mid-run, helper
            // restarting). Failing closed here would take away typing on
            // exactly the machines where it is the last resort.
            tracing::warn!(
                error = %e,
                "type_text focus pre-flight: AX query failed, allowing the keystrokes"
            );
            return None;
        }
    };

    match evaluate(focused.as_ref(), force) {
        Gate::Allow => None,
        Gate::Refuse(message) => Some(DesktopOutput {
            success: false,
            data: None,
            message: Some(super::recovery::with_hint(message)),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(role: &str) -> AxElement {
        AxElement {
            role: role.to_string(),
            pid: 1,
            ..Default::default()
        }
    }

    #[test]
    fn secure_field_is_refused() {
        let mut field = el("AXTextField");
        field.secure = Some(true);
        match evaluate(Some(&field), false) {
            Gate::Refuse(msg) => assert!(msg.contains("secure text field"), "{msg}"),
            Gate::Allow => panic!("typing into a password box must be refused"),
        }
    }

    #[test]
    fn secure_field_is_refused_even_with_force() {
        let mut field = el("AXTextField");
        field.secure = Some(true);
        assert!(matches!(evaluate(Some(&field), true), Gate::Refuse(_)));

        // Same on an older helper that reports only the subrole-as-role.
        let legacy = el("AXSecureTextField");
        assert!(matches!(evaluate(Some(&legacy), true), Gate::Refuse(_)));
    }

    #[test]
    fn nothing_focused_is_refused_with_an_actionable_message() {
        match evaluate(None, false) {
            Gate::Refuse(msg) => {
                assert!(msg.contains("set_value"), "must name the way out: {msg}");
                assert!(msg.contains("force:true"), "must name the override: {msg}");
            }
            Gate::Allow => panic!("blind typing with no focus must be refused"),
        }
    }

    #[test]
    fn nothing_focused_is_allowed_under_force() {
        assert_eq!(evaluate(None, true), Gate::Allow);
    }

    #[test]
    fn unsettable_non_editable_element_is_refused_and_names_the_role() {
        let mut button = el("AXButton");
        button.settable = Some(false);
        match evaluate(Some(&button), false) {
            Gate::Refuse(msg) => assert!(msg.contains("AXButton"), "{msg}"),
            Gate::Allow => panic!("a focused button takes no typed value"),
        }
        assert_eq!(evaluate(Some(&button), true), Gate::Allow);
    }

    #[test]
    fn unsettable_text_field_still_types() {
        // A text field whose AXValue cannot be written wholesale still accepts
        // keystrokes — this is exactly the case type_text exists for.
        let mut field = el("AXTextField");
        field.settable = Some(false);
        assert_eq!(evaluate(Some(&field), false), Gate::Allow);
    }

    #[test]
    fn unknown_metadata_fails_open() {
        // Older helper: no affordances on the wire at all.
        let plain = el("AXTextField");
        assert_eq!(plain.settable, None);
        assert_eq!(evaluate(Some(&plain), false), Gate::Allow);

        // Electron / canvas / terminal: focus legitimately sits on a container
        // that reports nothing. Refusing here would break real apps.
        assert_eq!(evaluate(Some(&el("AXWebArea")), false), Gate::Allow);
        assert_eq!(evaluate(Some(&el("AXGroup")), false), Gate::Allow);
        assert_eq!(evaluate(Some(&el("AXUnknown")), false), Gate::Allow);
    }
}
