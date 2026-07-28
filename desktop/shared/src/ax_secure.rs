//! Is this element a credential field? — the one answer, for every limb.
//!
//! `secure` is the single most consequential affordance an accessibility limb
//! reports: `builtin_tools::desktop::focus_gate` refuses `type_text` on it with
//! no `force` override, and `safe_value` / `redact_secure_values` withhold the
//! contents. A limb that gets this wrong types a credential into the wrong
//! place, which is not recoverable.
//!
//! Every platform has a *native* signal first — UIA's `IsPassword`, AT-SPI's
//! `Role::PasswordText`, the macOS `AXSecureTextField` role. This module is the
//! **second** signal, for the frameworks that never set the native one: Electron
//! and Qt custom editors routinely expose a masked field as an ordinary text
//! entry. It lived in `desktop/windows/src/ax.rs`, where the Linux AT-SPI limb —
//! which has exactly the same blind spot, on exactly the same frameworks —
//! could not reach it. It sits here for the same reason [`crate::ax_rank`] does:
//! two limbs need it, so the truth belongs in the crate they both depend on.
//!
//! Deliberately mechanical: fixed substrings over fixed fields, nothing else
//! read about the element (R7/P8).

/// AX roles (the mapped, cross-platform `"AX*"` vocabulary) that take typed text
/// and are therefore the only ones [`is_password_like`] is allowed to judge.
///
/// Restricting it matters: a heuristic that fired on any element whose label
/// merely contains "password" would refuse legitimate typing next to a
/// "Show password" checkbox, or inside a window titled "Password Manager".
const TEXT_ENTRY_ROLES: &[&str] = &["AXTextField", "AXComboBox"];

/// Substrings that mark a text entry as carrying a credential.
///
/// Mirrors the term list orca's runtimes use on both Windows and Linux, because
/// the failure being prevented is identical on both.
const CREDENTIAL_TERMS: &[&str] = &[
    "password",
    "passcode",
    "passphrase",
    "secret",
    "one-time code",
    "verification code",
];

/// Whether a text entry's labels mark it as a credential field.
///
/// `role` is the **mapped** AX role; `labels` are whatever identifying strings
/// the platform filled in — Name / `AutomationId` / `ClassName` on Windows,
/// name / description / accessible-id on AT-SPI.
///
/// Pure, so the judgement is unit-testable without a live desktop.
#[must_use]
pub fn is_password_like(role: &str, labels: &[&str]) -> bool {
    if !TEXT_ENTRY_ROLES.contains(&role) {
        return false;
    }
    let haystack = labels.join(" ").to_lowercase();
    if CREDENTIAL_TERMS.iter().any(|t| haystack.contains(t)) {
        return true;
    }
    // "pin" only as a whole word — "spinner", "pinned" and "shipping" are not
    // credential fields.
    haystack
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| w == "pin")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labelled_credential_fields_are_secure() {
        for label in [
            "Password",
            "password",
            "Enter your passphrase",
            "One-time code",
            "Verification code",
            "Client Secret",
            "PIN",
        ] {
            assert!(
                is_password_like("AXTextField", &[label, "", ""]),
                "{label:?} should read as a credential field"
            );
        }
        assert!(is_password_like("AXComboBox", &["Secret"]));
    }

    #[test]
    fn the_heuristic_reads_every_identifying_field_not_just_the_name() {
        // Electron and Qt often leave the name empty and put the meaning in the
        // automation id / class name (Windows) or the accessible id (AT-SPI).
        assert!(is_password_like("AXTextField", &["", "login-password", ""]));
        assert!(is_password_like("AXTextField", &["", "", "PasswordBox"]));
    }

    #[test]
    fn only_text_entry_roles_are_judged_by_label() {
        // The blast radius matters: `secure` is a hard block that `force` cannot
        // lift, so a checkbox labelled "Show password" or a group inside a
        // password manager must not silently disable typing everywhere.
        for role in [
            "AXCheckBox",
            "AXButton",
            "AXGroup",
            "AXStaticText",
            "AXWindow",
        ] {
            assert!(
                !is_password_like(role, &["Show password", "", ""]),
                "{role} must not be judged by its label"
            );
        }
    }

    #[test]
    fn pin_matches_only_as_a_whole_word() {
        assert!(is_password_like("AXTextField", &["Enter PIN", "", ""]));
        assert!(is_password_like("AXTextField", &["card-pin-entry"]));
        for benign in ["Spinner value", "Pinned tabs", "Shipping address"] {
            assert!(
                !is_password_like("AXTextField", &[benign, "", ""]),
                "{benign:?} is not a credential field"
            );
        }
    }

    #[test]
    fn an_ordinary_field_is_not_secure() {
        assert!(!is_password_like("AXTextField", &["Email address", "", ""]));
        assert!(!is_password_like("AXTextField", &[]));
        assert!(!is_password_like("AXTextField", &["", "", ""]));
    }
}
