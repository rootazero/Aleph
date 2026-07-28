//! Linux `AccessibilityCapability` backed by **AT-SPI2**.
//!
//! This is the Linux counterpart to the macOS AX layer and the Windows UI
//! Automation one. The cross-platform tool surface — `desktop_ax_snapshot`,
//! `desktop_ax_query_focused`, `desktop_ax_query_by_role`, `desktop_som`'s
//! Set-of-Marks grounding, `desktop_gui_locate`, and the `type_text` focus gate
//! — was already wired to `DesktopPlatform::ax()`, but Linux returned `None`,
//! so all of it degraded to "AX capability not available". This module fills
//! that gap.
//!
//! Three design choices keep it faithful to the existing architecture rather
//! than introducing a parallel one:
//!
//! 1. **The role vocabulary is macOS-shaped.** [`roles::atspi_role_to_ax_role`]
//!    maps every AT-SPI role onto the closest `"AX*"` string, so the shared
//!    consumers light up with zero changes — the same `INTERACTABLE_ROLES`
//!    allowlist simply starts matching. See that module for the table.
//! 2. **No handles cross a call boundary.** A locator re-walks the tree every
//!    time, ranked by [`aleph_desktop::rank_candidates`] — the same ranking the
//!    Windows limb and the macOS Swift helper use, so a locator means the same
//!    thing everywhere.
//! 3. **`ax()` may still be `None`.** `LinuxPlatform` only installs this
//!    capability when an accessibility bus is actually reachable
//!    ([`bus::bus_looks_reachable`]). A capability that is present and always
//!    failing would be worse than an absent one: the `type_text` gate treats an
//!    AX error as fail-open *and logs a warning*, so every keystroke on a
//!    desktop with accessibility switched off would carry a useless warning.
//!
//! # What this turns on
//!
//! The password refusal. `Role::PasswordText` maps to `"AXSecureTextField"` and
//! sets the `secure` affordance, which is exactly what
//! `builtin_tools::desktop::focus_gate` refuses on — with no override. Until
//! now that protection existed on macOS and Windows and simply did not exist on
//! Linux.

mod bus;
mod roles;
mod walk;

use std::collections::VecDeque;

use async_trait::async_trait;
use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::proxy_ext::ProxyExt;
use atspi::State;

use aleph_desktop::traits::AccessibilityCapability;
use aleph_desktop::{rank_candidates, DesktopError, RankCandidate, Result};
use aleph_protocol::desktop_bridge::methods::ax::{
    clamp_max_nodes, AxActionResult, AxElement, AxLocator, AxVerification, PerformActionParams,
    QueryByRoleParams, QueryFocusedParams, QueryListResult, QueryResult, QueryTreeParams,
    SetValueParams,
};

pub use bus::bus_looks_reachable;

use bus::{App, Bus};
use walk::Walk;

/// Depth bound for the walks that carry no explicit `max_depth`
/// (`query_by_role`, locator resolution). Deep enough to reach a control inside
/// a few layers of layout containers, bounded enough not to descend a web
/// document forever.
const SCAN_DEPTH: u32 = 12;

/// Depth bound for the focused-element search.
const FOCUS_DEPTH: u32 = 24;

/// `AccessibilityCapability` backed by AT-SPI2 over D-Bus.
///
/// Stateless: every call opens its own bus connection and drops it. See the
/// module docs for why no proxy is cached.
pub struct LinuxAccessibility {
    _private: (),
}

impl LinuxAccessibility {
    /// Build the capability if an accessibility bus looks reachable.
    ///
    /// Returns `None` on a desktop with accessibility switched off, so
    /// `DesktopPlatform::ax()` reports the honest absence rather than a
    /// capability that fails on every call.
    #[must_use]
    pub fn probe() -> Option<Self> {
        bus_looks_reachable().then_some(Self { _private: () })
    }
}

// ── Shared helpers ───────────────────────────────────────────────────────────

/// Resolve the application to work on, or explain why there is none.
async fn app_or_err(bus: &Bus, pid: Option<i32>) -> Result<App> {
    bus.app_for(pid).await?.ok_or_else(|| match pid {
        Some(pid) => DesktopError::NotAvailable(format!(
            "Process {pid} exposes no accessibility tree. Applications only appear on the AT-SPI \
             bus when their toolkit's accessibility bridge is loaded (GTK: `GTK_MODULES=gail:\
             atk-bridge`, Qt: `QT_ACCESSIBILITY=1`), and they must be restarted after enabling it."
        )),
        None => DesktopError::NotAvailable(
            "No frontmost application is reachable on the AT-SPI bus — either nothing holds \
             window focus, or the focused application's accessibility bridge is not loaded. Pass \
             an explicit pid, or fall back to screenshot + gui_locate."
                .into(),
        ),
    })
}

/// One element in a flattened locator-resolution scan.
struct Candidate {
    summary: RankCandidate,
    proxy: AccessibleProxy<'static>,
}

/// Flatten an application subtree into ranking candidates, breadth-first.
///
/// Breadth-first rather than depth-first on purpose: when a budget runs out,
/// what survives is the shallow structure — windows, toolbars, dialogs — which
/// is where a locator's target usually lives. A depth-first walk would spend
/// the whole budget inside the first container it entered.
async fn collect_candidates(
    bus: &Bus,
    root: AccessibleProxy<'static>,
    depth: u32,
) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut queue: VecDeque<(AccessibleProxy<'static>, u32)> = VecDeque::new();
    queue.push_back((root, depth));

    while let Some((proxy, remaining)) = queue.pop_front() {
        if out.len() >= walk::MAX_NODES {
            break;
        }
        let Ok(role) = proxy.get_role().await else {
            continue;
        };
        let title = proxy.name().await.ok().filter(|n| !n.is_empty());
        let center = center_of(&proxy).await;

        if let Some(center) = center {
            out.push(Candidate {
                summary: RankCandidate {
                    role: roles::atspi_role_to_ax_role(role).to_string(),
                    title,
                    center,
                },
                proxy: proxy.clone(),
            });
        }

        if remaining > 0 {
            if let Ok(children) = proxy.get_children().await {
                for child_ref in children {
                    if let Ok(child) = bus.proxy_for(&child_ref).await {
                        queue.push_back((child, remaining - 1));
                    }
                }
            }
        }
    }
    out
}

/// Center of an element's on-screen rectangle, or `None` when it has none.
///
/// An element with no rectangle is excluded from locator ranking entirely: a
/// `center` tiebreak against a fabricated origin would quietly favour whatever
/// happened to be nowhere. It runs the same reality checks the tree walk does —
/// not-showing widgets and impossible origins are both excluded — so a locator
/// and a snapshot agree on which elements exist.
async fn center_of(proxy: &AccessibleProxy<'static>) -> Option<(f64, f64)> {
    let showing = proxy
        .get_state()
        .await
        .is_ok_and(|s| s.contains(State::Showing));
    let ifaces = proxy.proxies().await.ok()?;
    let region = walk::extents_of(&ifaces, showing).await?;
    Some((
        region.x + region.width / 2.0,
        region.y + region.height / 2.0,
    ))
}

/// Resolve a locator to a single element, or say why it could not.
async fn resolve(bus: &Bus, locator: &AxLocator) -> Result<(AccessibleProxy<'static>, AxElement)> {
    let app = app_or_err(bus, locator.pid).await?;
    let pid = app.pid;
    let candidates = collect_candidates(bus, app.root, SCAN_DEPTH).await;

    let summaries: Vec<RankCandidate> = candidates.iter().map(|c| c.summary.clone()).collect();
    let index = rank_candidates(&summaries, locator).ok_or_else(|| {
        DesktopError::NotAvailable(format!(
            "No accessible element matched the locator (role={:?}, title={:?}) in pid {pid}. \
             Run desktop_ax_snapshot to see what the application actually exposes.",
            locator.role, locator.title
        ))
    })?;

    let proxy = candidates
        .into_iter()
        .nth(index)
        .expect("rank_candidates returns an in-range index")
        .proxy;

    // Re-read the matched element so the caller sees the element as it is now,
    // with its affordances — the ranking summary carries only role/title/center.
    let mut walk = Walk::new(bus, pid);
    let element = walk
        .element(&proxy, 0)
        .await
        .ok_or_else(|| DesktopError::PlatformError("Matched element became unreadable".into()))?;
    Ok((proxy, element))
}

/// Outcome of the focused-element search.
enum FocusScan {
    /// An element reports keyboard focus.
    Found(AccessibleProxy<'static>),
    /// The application reports focus semantics (it has focusable elements) and
    /// none of them currently holds focus.
    NothingFocused,
    /// The application says nothing about focus at all, so we do not know.
    Unknown,
}

/// Breadth-first search for the element holding keyboard focus.
///
/// AT-SPI has no "give me the focused element" query — focus is normally
/// observed as an event stream — so this walks the frontmost application's tree
/// looking for `State::Focused`.
///
/// The three-way outcome is load-bearing. `builtin_tools::desktop::focus_gate`
/// **refuses** typing on `Ok(None)` ("nothing is focused") but **allows** it on
/// `Err` ("we could not tell"), and on Linux those are genuinely different
/// situations: an application whose accessibility bridge is not loaded exposes
/// no focus state at all, and refusing to type into it would break the one
/// input path that does work there.
async fn find_focused(bus: &Bus, root: AccessibleProxy<'static>) -> FocusScan {
    let mut queue: VecDeque<(AccessibleProxy<'static>, u32)> = VecDeque::new();
    queue.push_back((root, FOCUS_DEPTH));
    let mut saw_focusable = false;
    let mut budget = walk::MAX_NODES;

    while let Some((proxy, remaining)) = queue.pop_front() {
        if budget == 0 {
            break;
        }
        budget -= 1;

        if let Ok(states) = proxy.get_state().await {
            if states.contains(State::Focused) {
                return FocusScan::Found(proxy);
            }
            if states.contains(State::Focusable) {
                saw_focusable = true;
            }
        }

        if remaining > 0 {
            if let Ok(children) = proxy.get_children().await {
                for child_ref in children {
                    if let Ok(child) = bus.proxy_for(&child_ref).await {
                        queue.push_back((child, remaining - 1));
                    }
                }
            }
        }
    }

    if saw_focusable {
        FocusScan::NothingFocused
    } else {
        FocusScan::Unknown
    }
}

/// Flatten an element tree in document order.
fn flatten(node: &AxElement, out: &mut Vec<AxElement>) {
    let mut copy = node.clone();
    copy.children = Vec::new();
    out.push(copy);
    for child in &node.children {
        flatten(child, out);
    }
}

// ── Capability ───────────────────────────────────────────────────────────────

#[async_trait]
impl AccessibilityCapability for LinuxAccessibility {
    /// AT-SPI has no "focused element of process P" query, so a `pid` narrows
    /// which application tree is scanned — which is the same answer, reached
    /// directly. Without one the frontmost application is scanned, as before.
    async fn query_focused(&self, params: QueryFocusedParams) -> Result<Option<AxElement>> {
        let bus = Bus::open().await?;
        let app = app_or_err(&bus, params.pid).await?;
        let pid = app.pid;
        match find_focused(&bus, app.root).await {
            FocusScan::Found(proxy) => {
                let mut walk = Walk::new(&bus, pid);
                Ok(walk.element(&proxy, 0).await)
            }
            FocusScan::NothingFocused => Ok(None),
            FocusScan::Unknown => Err(DesktopError::NotAvailable(format!(
                "The frontmost application (pid {pid}) reports no focus state over AT-SPI, so \
                 keyboard focus cannot be determined. Its toolkit accessibility bridge is \
                 probably not loaded."
            ))),
        }
    }

    async fn query_tree(&self, params: QueryTreeParams) -> Result<QueryResult> {
        let bus = Bus::open().await?;
        let app = app_or_err(&bus, params.pid).await?;
        let pid = app.pid;
        let mut walk = Walk::with_budget(&bus, pid, clamp_max_nodes(params.max_nodes) as usize);
        let element = walk.element(&app.root, params.max_depth).await;
        Ok(QueryResult {
            element,
            node_count: walk.spent(),
            truncated: walk.exhausted(),
        })
    }

    async fn query_by_role(&self, params: QueryByRoleParams) -> Result<QueryListResult> {
        let bus = Bus::open().await?;
        let app = app_or_err(&bus, params.pid).await?;
        let pid = app.pid;
        let mut walk = Walk::with_budget(&bus, pid, clamp_max_nodes(params.max_nodes) as usize);
        let tree = walk.element(&app.root, SCAN_DEPTH).await;
        let mut all = Vec::new();
        if let Some(tree) = tree.as_ref() {
            flatten(tree, &mut all);
        }
        Ok(QueryListResult {
            elements: all.into_iter().filter(|e| e.role == params.role).collect(),
            node_count: walk.spent(),
            truncated: walk.exhausted(),
        })
    }

    async fn set_value(&self, params: SetValueParams) -> Result<AxActionResult> {
        let bus = Bus::open().await?;
        let (proxy, matched) = resolve(&bus, &params.locator).await?;

        let ifaces = proxy
            .proxies()
            .await
            .map_err(|e| DesktopError::PlatformError(format!("AT-SPI interface query: {e}")))?;
        let editable = ifaces.editable_text().await.map_err(|_| {
            DesktopError::NotImplemented(format!(
                "The matched {} does not implement the AT-SPI EditableText interface, so its \
                 value cannot be written. Click it and use type_text instead.",
                matched.role
            ))
        })?;

        let performed = editable
            .set_text_contents(&params.value)
            .await
            .map_err(|e| DesktopError::PlatformError(format!("AT-SPI set_text_contents: {e}")))?;

        // Read back rather than trust the return value: a toolkit can accept the
        // call and clamp, reformat or reject the content.
        let actual = match ifaces.text().await {
            Ok(text) => text.get_text(0, -1).await.ok(),
            Err(_) => None,
        };
        let verification = Some(match &actual {
            Some(actual) if actual == &params.value => AxVerification {
                state: "verified".into(),
                reason: None,
                actual_preview: None,
            },
            Some(actual) => AxVerification {
                state: "unverified".into(),
                reason: Some("the element read back a different value".into()),
                actual_preview: Some(actual.chars().take(200).collect()),
            },
            None => AxVerification {
                state: "unverified".into(),
                reason: Some("the element exposes no readable text to verify against".into()),
                actual_preview: None,
            },
        });

        Ok(AxActionResult {
            performed,
            path: "accessibility".into(),
            matched: Some(matched),
            verification,
        })
    }

    async fn perform_action(&self, params: PerformActionParams) -> Result<AxActionResult> {
        let bus = Bus::open().await?;
        let (proxy, matched) = resolve(&bus, &params.locator).await?;

        let ifaces = proxy
            .proxies()
            .await
            .map_err(|e| DesktopError::PlatformError(format!("AT-SPI interface query: {e}")))?;
        let action = ifaces.action().await.map_err(|_| {
            DesktopError::NotImplemented(format!(
                "The matched {} implements no AT-SPI Action interface, so it has no native action \
                 to trigger. Click its center instead.",
                matched.role
            ))
        })?;

        // Canonical (untranslated) names — exactly the strings a snapshot
        // reported, see `walk::canonical_action_names`. Matching against
        // `GetActions` here would compare an English request against a localized
        // label ("点击") and fail on every non-English desktop, while the
        // snapshot the model read from said "click".
        let available = walk::canonical_action_names(&action).await;

        let index = pick_action(&available, &params.action).ok_or_else(|| {
            DesktopError::NotImplemented(format!(
                "The matched {} does not support '{}'. It offers: {}. Pass one of those verbatim.",
                matched.role,
                params.action,
                if available.is_empty() {
                    "nothing".to_string()
                } else {
                    available.join(", ")
                }
            ))
        })?;

        let performed = action
            .do_action(i32::try_from(index).unwrap_or(0))
            .await
            .map_err(|e| DesktopError::PlatformError(format!("AT-SPI do_action: {e}")))?;

        Ok(AxActionResult {
            performed,
            path: "accessibility".into(),
            matched: Some(matched),
            verification: None,
        })
    }
}

/// Choose which of an element's actions answers to `wanted`.
///
/// Accepts an AT-SPI action name verbatim (what a snapshot reports on Linux)
/// and the macOS `"AX*"` spelling (what a cross-platform prompt says), so a
/// model that learned `AXPress` on one platform is not wrong on this one.
///
/// Pure, so the whole alias table is testable without a desktop.
#[must_use]
fn pick_action(available: &[String], wanted: &str) -> Option<usize> {
    // Verbatim first: an element's own reported name always wins.
    if let Some(i) = available
        .iter()
        .position(|a| a.eq_ignore_ascii_case(wanted))
    {
        return Some(i);
    }
    // Then the macOS alias table, in preference order.
    for alias in roles::ax_action_aliases(wanted)? {
        if let Some(i) = available.iter().position(|a| a.eq_ignore_ascii_case(alias)) {
            return Some(i);
        }
    }
    None
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn an_elements_own_action_name_matches_verbatim() {
        let available = names(&["click", "press"]);
        assert_eq!(pick_action(&available, "click"), Some(0));
        assert_eq!(
            pick_action(&available, "Click"),
            Some(0),
            "case-insensitive"
        );
        assert_eq!(pick_action(&available, "press"), Some(1));
    }

    #[test]
    fn the_macos_spelling_resolves_through_the_alias_table() {
        // A prompt written against the cross-platform tool description says
        // AXPress; AT-SPI calls it "click".
        assert_eq!(pick_action(&names(&["click"]), "AXPress"), Some(0));
        assert_eq!(pick_action(&names(&["activate"]), "AXPress"), Some(0));
        assert_eq!(pick_action(&names(&["menu"]), "AXShowMenu"), Some(0));
    }

    #[test]
    fn aliases_respect_their_preference_order() {
        // "click" is preferred over "activate" when both exist.
        let available = names(&["activate", "click"]);
        assert_eq!(pick_action(&available, "AXPress"), Some(1));
    }

    #[test]
    fn an_unsupported_action_is_none_rather_than_the_first_one() {
        // Falling back to index 0 would silently press a button when the model
        // asked to show its menu.
        assert_eq!(pick_action(&names(&["click"]), "AXShowMenu"), None);
        assert_eq!(pick_action(&names(&["click"]), "AXRaise"), None);
        assert_eq!(pick_action(&[], "AXPress"), None);
    }

    #[test]
    fn flatten_visits_every_node_in_document_order_without_duplicating_children() {
        let leaf = |role: &str| AxElement {
            role: role.into(),
            ..Default::default()
        };
        let tree = AxElement {
            role: "AXWindow".into(),
            children: vec![
                AxElement {
                    role: "AXGroup".into(),
                    children: vec![leaf("AXButton")],
                    ..Default::default()
                },
                leaf("AXTextField"),
            ],
            ..Default::default()
        };

        let mut out = Vec::new();
        flatten(&tree, &mut out);
        let roles: Vec<&str> = out.iter().map(|e| e.role.as_str()).collect();
        assert_eq!(
            roles,
            vec!["AXWindow", "AXGroup", "AXButton", "AXTextField"]
        );
        // Flattened nodes must not carry their children too, or a caller
        // filtering by role gets the same element several times over.
        assert!(out.iter().all(|e| e.children.is_empty()));
    }

    #[test]
    fn probe_matches_the_reachability_answer() {
        assert_eq!(LinuxAccessibility::probe().is_some(), bus_looks_reachable());
    }

    /// Live smoke test against a real accessibility bus, skipped where there is
    /// none (CI, a desktop with accessibility switched off) so it never becomes
    /// a flaky gate.
    ///
    /// It pins the property no fixture test can reach: that what comes back
    /// speaks the **macOS role vocabulary**, because that is the whole contract
    /// this limb exists to satisfy — every shared consumer filters on `"AX*"`
    /// strings, and an AT-SPI role name leaking through would make the element
    /// invisible to all of them.
    #[tokio::test]
    async fn live_tree_speaks_the_ax_role_vocabulary() {
        let Ok(bus) = Bus::open().await else {
            return; // no accessibility bus here
        };
        let Ok(apps) = bus.apps().await else {
            return;
        };
        // The app with the most on-screen structure, so the assertions below run
        // against a real window rather than a session daemon with an empty tree.
        let Some(app) = apps.into_iter().next() else {
            return; // bus is up but no application has registered
        };

        let mut walk = Walk::new(&bus, app.pid);
        let Some(tree) = walk.element(&app.root, 4).await else {
            return;
        };

        let mut all = Vec::new();
        flatten(&tree, &mut all);
        for element in &all {
            assert!(
                element.role.starts_with("AX"),
                "role {:?} is not in the AX vocabulary every consumer filters on",
                element.role
            );
            assert_eq!(element.pid, app.pid, "every node carries its owning pid");
            if let Some(b) = &element.bounds {
                assert!(
                    b.width > 0.0 && b.height > 0.0,
                    "a reported rectangle must be usable: {b:?}"
                );
            }
        }
    }
}
