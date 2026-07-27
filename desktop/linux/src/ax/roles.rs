//! AT-SPI role → macOS `"AX*"` role vocabulary.
//!
//! The whole cross-platform consumer surface — `desktop_som`'s Set-of-Marks
//! grounding, `desktop_ax_snapshot`, `gui_locate`, and the `type_text` focus
//! gate — filters elements by macOS `"AX*"` role strings. Windows already maps
//! its UIA `ControlType` ids onto that vocabulary for exactly this reason; this
//! is the AT-SPI half.
//!
//! The payoff is that Linux lights up those tools with **zero changes to any
//! consumer**: the same `INTERACTABLE_ROLES` allowlist simply starts matching.
//!
//! Two mappings carry weight beyond naming:
//!
//! * [`Role::PasswordText`] → `"AXSecureTextField"`. That single line is what
//!   turns on the `type_text` password refusal on Linux, because the gate
//!   recognises a password box by exactly that role (and by the `secure`
//!   affordance, which [`is_secure`] sets from the same source).
//! * Containers map onto their non-interactable AX equivalents deliberately.
//!   They show up in a full tree dump but never get a clickable mark, which is
//!   what keeps a snapshot a short list of things a user can act on rather than
//!   a wall of panels.

use atspi::Role;

/// Map an AT-SPI role onto the closest macOS `"AX*"` role string.
///
/// Unknown and exotic roles fall back to `"AXUnknown"` — present in a tree
/// dump, never in the actionable set.
#[must_use]
pub fn atspi_role_to_ax_role(role: Role) -> &'static str {
    match role {
        // ── interactable (must match INTERACTABLE_ROLES) ──
        // AT-SPI spells "push button" `Button`; there is no separate plain
        // "button" role.
        Role::Button | Role::PushButtonMenu => "AXButton",
        Role::ToggleButton | Role::CheckBox => "AXCheckBox",
        // AX models a tab group as a radio group, so a tab is selected the way
        // a radio button is — and belongs in the actionable set.
        Role::RadioButton | Role::PageTab => "AXRadioButton",
        Role::CheckMenuItem | Role::RadioMenuItem | Role::MenuItem | Role::TearoffMenuItem => {
            "AXMenuItem"
        }
        Role::ComboBox => "AXComboBox",
        Role::PasswordText => "AXSecureTextField",
        Role::Entry | Role::Text | Role::Editbar | Role::Autocomplete => "AXTextField",
        Role::Link => "AXLink",
        Role::Slider | Role::Dial => "AXSlider",
        Role::SpinButton => "AXIncrementor",
        Role::ColorChooser => "AXColorWell",

        // ── containers and decoration (deliberately not actionable) ──
        Role::Frame | Role::Window | Role::Dialog | Role::Alert | Role::InternalFrame => "AXWindow",
        Role::Panel
        | Role::Filler
        | Role::Grouping
        | Role::Section
        | Role::Form
        | Role::RootPane
        | Role::LayeredPane
        | Role::SplitPane
        | Role::Viewport
        | Role::ScrollPane => "AXGroup",
        Role::Menu | Role::PopupMenu => "AXMenu",
        Role::MenuBar => "AXMenuBar",
        Role::ToolBar => "AXToolbar",
        Role::List | Role::ListBox => "AXList",
        Role::Tree | Role::TreeTable => "AXOutline",
        Role::ListItem | Role::TreeItem | Role::TableRow | Role::TableCell => "AXRow",
        Role::Table => "AXTable",
        Role::PageTabList => "AXTabGroup",
        Role::Label | Role::Static | Role::Paragraph | Role::Heading | Role::Caption => {
            "AXStaticText"
        }
        Role::Image | Role::Icon => "AXImage",
        Role::ScrollBar => "AXScrollBar",
        Role::ProgressBar | Role::LevelBar => "AXProgressIndicator",
        Role::Separator => "AXSplitter",
        Role::StatusBar => "AXGroup",
        Role::ToolTip => "AXStaticText",
        Role::Application => "AXApplication",
        Role::DocumentWeb
        | Role::DocumentText
        | Role::DocumentFrame
        | Role::DocumentEmail
        | Role::DocumentSpreadsheet
        | Role::DocumentPresentation => "AXWebArea",
        Role::Terminal => "AXTextArea",

        _ => "AXUnknown",
    }
}

/// `true` for a role whose content is masked — a password box.
///
/// Feeds `AxElement.secure`, which is one of the two signals the `type_text`
/// gate refuses on (the other is the role string this same module produces).
#[must_use]
pub const fn is_secure(role: Role) -> bool {
    matches!(role, Role::PasswordText)
}

/// `true` for a role whose textual value is worth reading.
///
/// Fetching the value costs a D-Bus round trip per node, so a tree walk asks
/// only where a value is meaningful. A password box is excluded here as well as
/// redacted downstream: the safest place not to leak a secret is to never read
/// it across the bus in the first place.
#[must_use]
pub const fn has_readable_value(role: Role) -> bool {
    matches!(
        role,
        Role::Entry
            | Role::Text
            | Role::Editbar
            | Role::Autocomplete
            | Role::ComboBox
            | Role::Terminal
            | Role::Paragraph
            | Role::Heading
            | Role::Label
            | Role::Static
    )
}

/// `true` for a role a user can act on — the set worth asking for actions and
/// geometry on, and the set `desktop_som` will mark.
///
/// Mirrors `builtin_tools::desktop::interactable::INTERACTABLE_ROLES`, one
/// level up in the mapping: if this says yes, the AX role it maps to is in that
/// allowlist. [`interactable_roles_map_into_the_actionable_ax_set`] pins the
/// two together so they cannot drift apart silently.
///
/// [`interactable_roles_map_into_the_actionable_ax_set`]: self
#[must_use]
pub const fn is_interactable(role: Role) -> bool {
    matches!(
        role,
        Role::Button
            | Role::PushButtonMenu
            | Role::ToggleButton
            | Role::CheckBox
            | Role::RadioButton
            | Role::PageTab
            | Role::CheckMenuItem
            | Role::RadioMenuItem
            | Role::MenuItem
            | Role::TearoffMenuItem
            | Role::ComboBox
            | Role::PasswordText
            | Role::Entry
            | Role::Text
            | Role::Editbar
            | Role::Autocomplete
            | Role::Link
            | Role::Slider
            | Role::Dial
            | Role::SpinButton
            | Role::ColorChooser
    )
}

/// AT-SPI action names that answer to a macOS `"AX*"` action, in preference
/// order.
///
/// The model is told to pass an element's own reported action verbatim, and on
/// Linux those are AT-SPI names (`"click"`, `"activate"`). This alias table
/// additionally accepts the macOS spellings so a prompt written against the
/// cross-platform tool description still works here — and so `AXPress` means
/// the same thing on all three platforms.
///
/// Returns `None` for an action with no known equivalent, which the caller
/// turns into an honest `NotImplemented` rather than a silent no-op.
#[must_use]
pub fn ax_action_aliases(action: &str) -> Option<&'static [&'static str]> {
    match action {
        "AXPress" | "AXConfirm" => Some(&["click", "press", "activate", "jump", "open"]),
        "AXShowMenu" => Some(&["show menu", "menu", "expand or contract", "expand"]),
        "AXIncrement" => Some(&["increment", "increase"]),
        "AXDecrement" => Some(&["decrement", "decrease"]),
        _ => None,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// The roles the shared consumer treats as actionable
    /// (`builtin_tools::desktop::interactable::INTERACTABLE_ROLES`). Copied
    /// here because that list lives in the core crate, which the limbs must not
    /// depend on (R1); the test below is what keeps the copy honest.
    const CONSUMER_INTERACTABLE_ROLES: &[&str] = &[
        "AXButton",
        "AXMenuButton",
        "AXPopUpButton",
        "AXMenuItem",
        "AXMenuBarItem",
        "AXCheckBox",
        "AXRadioButton",
        "AXDisclosureTriangle",
        "AXTextField",
        "AXTextArea",
        "AXSearchField",
        "AXSecureTextField",
        "AXComboBox",
        "AXLink",
        "AXSlider",
        "AXIncrementor",
        "AXStepper",
        "AXColorWell",
        "AXSegmentedControl",
    ];

    #[test]
    fn a_password_box_is_a_secure_text_field() {
        // This is the line that turns on the type_text password refusal on
        // Linux. Both signals the gate reads must agree.
        assert_eq!(
            atspi_role_to_ax_role(Role::PasswordText),
            "AXSecureTextField"
        );
        assert!(is_secure(Role::PasswordText));
        assert!(!is_secure(Role::Entry));
        assert!(!is_secure(Role::Text));
    }

    #[test]
    fn a_password_boxs_value_is_never_read_across_the_bus() {
        assert!(!has_readable_value(Role::PasswordText));
    }

    #[test]
    fn interactable_roles_map_into_the_actionable_ax_set() {
        // Every role this module calls interactable must map to a role the
        // consumer's allowlist actually contains — otherwise the walk pays for
        // actions and geometry on elements that can never be marked.
        for role in [
            Role::Button,
            Role::PushButtonMenu,
            Role::ToggleButton,
            Role::CheckBox,
            Role::RadioButton,
            Role::PageTab,
            Role::CheckMenuItem,
            Role::RadioMenuItem,
            Role::MenuItem,
            Role::TearoffMenuItem,
            Role::ComboBox,
            Role::PasswordText,
            Role::Entry,
            Role::Text,
            Role::Editbar,
            Role::Autocomplete,
            Role::Link,
            Role::Slider,
            Role::Dial,
            Role::SpinButton,
            Role::ColorChooser,
        ] {
            assert!(is_interactable(role), "{role:?} should be interactable");
            let ax = atspi_role_to_ax_role(role);
            assert!(
                CONSUMER_INTERACTABLE_ROLES.contains(&ax),
                "{role:?} maps to {ax}, which the consumer allowlist does not contain"
            );
        }
    }

    #[test]
    fn containers_are_visible_in_a_tree_but_never_actionable() {
        for role in [
            Role::Panel,
            Role::Filler,
            Role::Frame,
            Role::Window,
            Role::MenuBar,
            Role::ToolBar,
            Role::List,
            Role::Label,
            Role::Image,
            Role::StatusBar,
        ] {
            assert!(!is_interactable(role), "{role:?} must not be actionable");
            let ax = atspi_role_to_ax_role(role);
            assert_ne!(ax, "AXUnknown", "{role:?} deserves a real container role");
            assert!(
                !CONSUMER_INTERACTABLE_ROLES.contains(&ax),
                "{role:?} maps to the actionable role {ax}"
            );
        }
    }

    #[test]
    fn unmapped_roles_fall_back_rather_than_guessing() {
        assert_eq!(atspi_role_to_ax_role(Role::Invalid), "AXUnknown");
        assert_eq!(atspi_role_to_ax_role(Role::Unknown), "AXUnknown");
        assert_eq!(atspi_role_to_ax_role(Role::Animation), "AXUnknown");
    }

    #[test]
    fn a_web_document_reads_as_a_web_area_so_the_focus_gate_fails_open() {
        // The gate treats AXWebArea as "reports nothing, allow typing"; a
        // browser's document must land there rather than on a role that refuses.
        assert_eq!(atspi_role_to_ax_role(Role::DocumentWeb), "AXWebArea");
        assert_eq!(atspi_role_to_ax_role(Role::DocumentText), "AXWebArea");
    }

    #[test]
    fn a_terminal_is_a_text_area_not_an_unknown() {
        // Terminals legitimately take keystrokes while reporting no settable
        // value; AXTextArea is in the editable-roles list the gate allows.
        assert_eq!(atspi_role_to_ax_role(Role::Terminal), "AXTextArea");
    }

    #[test]
    fn ax_action_aliases_cover_the_advertised_actions() {
        assert!(ax_action_aliases("AXPress").unwrap().contains(&"click"));
        assert!(ax_action_aliases("AXShowMenu").is_some());
        assert!(ax_action_aliases("AXIncrement").is_some());
        // An action with no equivalent must say so rather than silently no-op.
        assert!(ax_action_aliases("AXRaise").is_none());
        assert!(ax_action_aliases("").is_none());
    }
}
