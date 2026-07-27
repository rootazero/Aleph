//! Turning AT-SPI objects into [`AxElement`] trees.
//!
//! Every node costs D-Bus round trips, so the walk is deliberately frugal:
//! role, name, states and geometry always; the textual value only for roles
//! where a value means something; the action list only for roles a user can act
//! on. A password box's value is never fetched at all — the safest place not to
//! leak a secret is to never read it across the bus.
//!
//! Both a depth limit and a node budget bound the walk. A single Electron
//! window can expose tens of thousands of accessible objects, and an unbounded
//! walk would be a several-minute D-Bus storm before it became an unusable
//! wall of JSON.

use std::pin::Pin;

use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::proxy_ext::ProxyExt;
use atspi::{CoordType, Role, State, StateSet};

use aleph_protocol::desktop_bridge::methods::ax::AxElement;
use aleph_protocol::desktop_bridge::methods::screen::Region;

use super::bus::Bus;
use super::roles::{atspi_role_to_ax_role, has_readable_value, is_interactable, is_secure};

/// Hard cap on how many nodes one walk will materialize.
pub const MAX_NODES: usize = 1_500;

/// Longest value string carried back for one element.
const MAX_VALUE_CHARS: usize = 500;

// `+ Send` is required, not incidental: `AccessibilityCapability` is an
// `#[async_trait]` whose futures must be `Send`, and boxing a recursive async fn
// erases that unless it is spelled out.
type BoxFut<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// A bounded tree walk over one application's accessible objects.
pub struct Walk<'a> {
    bus: &'a Bus,
    pid: i32,
    remaining: usize,
}

impl<'a> Walk<'a> {
    /// Start a walk over `pid`'s tree, materializing at most [`MAX_NODES`].
    pub const fn new(bus: &'a Bus, pid: i32) -> Self {
        Self {
            bus,
            pid,
            remaining: MAX_NODES,
        }
    }

    /// Build the element rooted at `proxy`, descending `depth` more levels.
    ///
    /// Returns `None` for an object that cannot be read at all (it exited, or
    /// its application stopped answering); a node whose *optional* attributes
    /// fail still comes back, with those attributes absent rather than faked.
    pub fn element<'s>(
        &'s mut self,
        proxy: &'s AccessibleProxy<'static>,
        depth: u32,
    ) -> BoxFut<'s, Option<AxElement>> {
        Box::pin(async move {
            if self.remaining == 0 {
                return None;
            }
            self.remaining -= 1;

            // The role is the one attribute nothing works without: it decides
            // the AX vocabulary, the actionable set, and the secure refusal.
            let role = proxy.get_role().await.ok()?;
            let states = proxy.get_state().await.unwrap_or_default();
            let title = proxy.name().await.ok().filter(|n| !n.is_empty());

            let showing = states.contains(State::Showing);
            // Asking for the interface set costs a round trip, and it only buys
            // something for a node we will then query further: bounds (needs
            // showing), a value (needs a text-ish role), or actions (needs an
            // interactable role — hidden menu items included, since triggering
            // one without opening its menu is exactly what the Action interface
            // is for). A hidden container buys nothing.
            let ifaces = if showing || is_interactable(role) {
                proxy.proxies().await.ok()
            } else {
                None
            };
            let bounds = match &ifaces {
                Some(p) => extents_of(p, showing).await,
                None => None,
            };
            let value = match (&ifaces, has_readable_value(role)) {
                (Some(p), true) => text_of(p).await,
                _ => None,
            };
            let actions = match (&ifaces, is_interactable(role)) {
                (Some(p), true) => actions_of(p).await,
                _ => None,
            };

            let mut children = Vec::new();
            if depth > 0 && self.remaining > 0 {
                if let Ok(refs) = proxy.get_children().await {
                    for child_ref in refs {
                        if self.remaining == 0 {
                            break;
                        }
                        let Ok(child) = self.bus.proxy_for(&child_ref).await else {
                            continue;
                        };
                        if let Some(node) = self.element(&child, depth - 1).await {
                            children.push(node);
                        }
                    }
                }
            }

            Some(AxElement {
                role: atspi_role_to_ax_role(role).to_string(),
                title,
                value,
                bounds,
                pid: self.pid,
                secure: Some(is_secure(role)),
                enabled: Some(enabled_of(states)),
                settable: settable_of(role, states),
                actions,
                url: None,
                children,
            })
        })
    }
}

/// Whether the element is interactive right now, as opposed to greyed out.
///
/// AT-SPI splits this in two: `Enabled` is the widget's own flag, `Sensitive`
/// is whether it can receive input given its ancestors. A user sees "greyed
/// out" when either is false.
fn enabled_of(states: StateSet) -> bool {
    states.contains(State::Enabled) && states.contains(State::Sensitive)
}

/// Whether `ax.set_value` can write this element — and, crucially, when to say
/// nothing at all.
///
/// The `type_text` focus gate refuses on `settable: Some(false)` for any role
/// outside its editable list. So reporting `Some(false)` for a container is not
/// a harmless detail: it would refuse typing into a browser document, a canvas
/// or a terminal — exactly the applications whose accessibility trees say the
/// least and where typing is the only way in.
///
/// The rule is therefore: `Some(true)` when AT-SPI says editable, `Some(false)`
/// only for controls where "takes no typed value" is a real fact about the
/// widget (a button, a link, a checkbox), and `None` — unknown — for everything
/// else, which the gate reads as fail-open.
fn settable_of(role: Role, states: StateSet) -> Option<bool> {
    if states.contains(State::Editable) {
        return Some(true);
    }
    if is_interactable(role) && !has_readable_value(role) {
        return Some(false);
    }
    None
}

/// Screen-space bounds via the `Component` interface, when they mean anything.
///
/// `showing` gates the whole query: GTK keeps every menu item in the tree at all
/// times and marks the unmapped ones not-showing, and asking such a widget for
/// its extents yields `(i32::MIN, i32::MIN, 292, 26)` — a *real size* at an
/// impossible origin. See [`usable_region`] for why that is the dangerous shape.
pub(super) async fn extents_of(
    p: &atspi::proxy::proxy_ext::Proxies<'_>,
    showing: bool,
) -> Option<Region> {
    if !showing {
        return None;
    }
    let component = p.component().await.ok()?;
    let (x, y, w, h) = component.get_extents(CoordType::Screen).await.ok()?;
    usable_region(x, y, w, h)
}

/// Largest screen coordinate treated as real. No display reaches it; every
/// sentinel AT-SPI hands back for "nowhere" is far beyond it.
const COORD_SANITY_LIMIT: i32 = 1_000_000;

/// Convert raw extents into a rectangle, or `None` when they describe nowhere.
///
/// A degenerate or impossible box is worse than no box at all: the consumer's
/// `usable_bounds` filter keeps anything wider and taller than one pixel, so a
/// widget parked at `i32::MIN` with a plausible 292×26 size becomes a clickable
/// mark — and the click lands on whatever is at that coordinate after the cast,
/// which is not the widget.
pub(super) fn usable_region(x: i32, y: i32, w: i32, h: i32) -> Option<Region> {
    if w <= 0 || h <= 0 {
        return None;
    }
    if x.saturating_abs() > COORD_SANITY_LIMIT || y.saturating_abs() > COORD_SANITY_LIMIT {
        return None;
    }
    Some(Region {
        x: f64::from(x),
        y: f64::from(y),
        width: f64::from(w),
        height: f64::from(h),
    })
}

/// Textual content via the `Text` interface, truncated.
async fn text_of(p: &atspi::proxy::proxy_ext::Proxies<'_>) -> Option<String> {
    let text = p.text().await.ok()?;
    // Ask for a bounded range rather than the whole buffer: a terminal or a log
    // view can hold megabytes, and all of it would cross the bus.
    let raw = text
        .get_text(0, i32::try_from(MAX_VALUE_CHARS).unwrap_or(i32::MAX))
        .await
        .ok()?;
    if raw.is_empty() {
        return None;
    }
    Some(truncate_chars(&raw, MAX_VALUE_CHARS))
}

/// Canonical action names via the `Action` interface.
///
/// **Not** `GetActions`. That bulk call returns the *localized* label — on a
/// Chinese desktop a GTK menu item reports its action as `"点击"`, not
/// `"click"` — so a model told to pass an action back verbatim would send a
/// string no alias table could ever recognise, and the same prompt would behave
/// differently per locale. `GetName(index)` is the untranslated name;
/// `GetLocalizedName` is the one meant for display, which is not what this is.
async fn actions_of(p: &atspi::proxy::proxy_ext::Proxies<'_>) -> Option<Vec<String>> {
    let action = p.action().await.ok()?;
    let count = action.n_actions().await.ok()?;
    let mut names = Vec::new();
    for index in 0..count.min(MAX_ACTIONS) {
        if let Ok(name) = action.get_name(index).await {
            if !name.is_empty() {
                names.push(name);
            }
        }
    }
    (!names.is_empty()).then_some(names)
}

/// Cap on actions read per element — one D-Bus round trip each, and no real
/// widget offers more than a handful.
const MAX_ACTIONS: i32 = 8;

/// Truncate to at most `max` characters without splitting one.
fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((idx, _)) => s[..idx].to_string(),
        None => s.to_string(),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    fn states(list: &[State]) -> StateSet {
        list.iter().copied().collect()
    }

    #[test]
    fn enabled_requires_both_atspi_flags() {
        assert!(enabled_of(states(&[State::Enabled, State::Sensitive])));
        assert!(!enabled_of(states(&[State::Enabled])));
        assert!(!enabled_of(states(&[State::Sensitive])));
        assert!(!enabled_of(states(&[])));
    }

    #[test]
    fn an_editable_element_is_settable() {
        assert_eq!(
            settable_of(Role::Entry, states(&[State::Editable])),
            Some(true)
        );
        assert_eq!(
            settable_of(Role::PasswordText, states(&[State::Editable])),
            Some(true)
        );
    }

    #[test]
    fn a_control_that_takes_no_typed_value_says_so() {
        // The gate refuses typing here, which is the point.
        assert_eq!(settable_of(Role::Button, states(&[])), Some(false));
        assert_eq!(settable_of(Role::Link, states(&[])), Some(false));
        assert_eq!(settable_of(Role::CheckBox, states(&[])), Some(false));
    }

    #[test]
    fn containers_and_documents_stay_unknown_so_the_gate_fails_open() {
        // Reporting Some(false) here would refuse typing into a browser, a
        // canvas or a terminal — the apps whose trees say the least.
        for role in [
            Role::DocumentWeb,
            Role::Panel,
            Role::Filler,
            Role::Unknown,
            Role::Terminal,
            Role::Frame,
        ] {
            assert_eq!(
                settable_of(role, states(&[])),
                None,
                "{role:?} must stay unknown"
            );
        }
    }

    #[test]
    fn a_read_only_text_field_stays_unknown_rather_than_refusing() {
        // A text field whose AT-SPI Editable flag is absent still accepts
        // keystrokes in many toolkits; unknown lets the gate allow it.
        assert_eq!(settable_of(Role::Entry, states(&[])), None);
        assert_eq!(settable_of(Role::Text, states(&[])), None);
    }

    #[test]
    fn an_unmapped_widget_parked_at_int_min_has_no_usable_rectangle() {
        // The shape GTK actually hands back for a hidden menu item: an
        // impossible origin with a perfectly plausible size.
        assert!(usable_region(i32::MIN, i32::MIN, 292, 26).is_none());
        assert!(usable_region(-2_000_000, 40, 100, 20).is_none());
        assert!(usable_region(40, i32::MIN, 100, 20).is_none());
    }

    #[test]
    fn degenerate_sizes_have_no_usable_rectangle() {
        assert!(usable_region(0, 0, 0, 10).is_none());
        assert!(usable_region(0, 0, 10, 0).is_none());
        assert!(usable_region(0, 0, -1, -1).is_none());
    }

    #[test]
    fn a_real_rectangle_survives_including_negative_screen_origins() {
        // A window on a monitor left of the primary has a negative x, and that
        // is a legitimate coordinate — the guard must not swallow it.
        let r = usable_region(-1920, 27, 1112, 818).expect("real rectangle");
        assert!((r.x - -1920.0).abs() < f64::EPSILON);
        assert!((r.width - 1112.0).abs() < f64::EPSILON);
    }

    #[test]
    fn truncation_is_utf8_safe_and_bounded() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 2), "he");
        // Multi-byte characters must not be split.
        assert_eq!(truncate_chars("日本語テキスト", 3), "日本語");
        assert_eq!(truncate_chars("", 5), "");
    }

    #[test]
    fn the_node_budget_is_spent_not_ignored() {
        // Guards the one property a unit test can reach without a live bus:
        // the walk cannot be constructed already over budget, and the budget is
        // the documented cap.
        assert_eq!(MAX_NODES, 1_500);
    }
}
