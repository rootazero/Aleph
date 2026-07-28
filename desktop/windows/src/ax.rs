//! Windows `AccessibilityCapability` implementation backed by **UI Automation**
//! (`IUIAutomation`).
//!
//! This is the Windows counterpart to the macOS [`BridgeAccessibility`] AX
//! layer. The cross-platform tool surface (`desktop_ax_query_tree`,
//! `desktop_ax_query_focused`, `desktop_ax_query_by_role`, and crucially
//! `desktop_som`'s Set-of-Marks grounding) was already wired to
//! `DesktopPlatform::ax()`, but Windows returned `None`, so every one of those
//! tools degraded to "AX capability not available". This module fills that gap.
//!
//! Two deliberate design choices keep it faithful to the existing architecture
//! rather than introducing a parallel one:
//!
//! 1. **Role vocabulary is macOS-shaped.** The shared consumer
//!    (`builtin_tools/desktop/interactable.rs`) filters elements by macOS
//!    `"AX*"` role strings. So [`control_type_to_ax_role`] maps each UIA
//!    `ControlType` onto the closest `"AX*"` role. The result: `desktop_som`
//!    and the snapshot tools light up on Windows with **zero changes to any
//!    consumer** — the same `INTERACTABLE_ROLES` allowlist just starts matching.
//!
//! 2. **No COM state on the struct.** COM interface pointers are neither `Send`
//!    nor `Sync`, but `AccessibilityCapability` requires both. Every call
//!    therefore does its COM work inside a `spawn_blocking` closure that
//!    initializes COM (MTA), builds a fresh `IUIAutomation`, walks the tree,
//!    and drops all pointers before returning a plain owned [`AxElement`]. This
//!    is slightly less efficient than caching the automation object, but these
//!    are interactive, human-paced queries — correctness and thread-safety win.
//!
//! [`BridgeAccessibility`]: ../../macos/src/ax.rs

use async_trait::async_trait;

use aleph_desktop::traits::AccessibilityCapability;
use aleph_desktop::{DesktopError, Result};
use aleph_protocol::desktop_bridge::methods::ax::{
    AxActionResult, AxElement, PerformActionParams, QueryByRoleParams, QueryTreeParams,
    SetValueParams,
};

// ── UIA ControlType → macOS AX role mapping (pure, host-testable) ────────────

#[cfg_attr(not(any(windows, test)), allow(unused_imports))]
pub use role_map::control_type_to_ax_role;

// The mapping is consumed by the `cfg(windows)` `imp` module (production) and by
// the `cfg(test)` unit tests. On a host non-test *library* build neither
// consumer is compiled, so the items look dead — a pure `cfg` artifact. A
// module-scoped `allow` silences that one configuration without masking real
// dead code elsewhere in the crate.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
mod role_map {
    // Well-known UI Automation `ControlType` ids. These integer constants are
    // stable parts of the UIA contract (`UIA_*ControlTypeId`); we name them
    // locally so the mapping is readable and unit-testable on any host without
    // pulling in the `windows` crate.
    const CT_BUTTON: i32 = 50000;
    const CT_CHECKBOX: i32 = 50002;
    const CT_COMBOBOX: i32 = 50003;
    const CT_EDIT: i32 = 50004;
    const CT_HYPERLINK: i32 = 50005;
    const CT_IMAGE: i32 = 50006;
    const CT_LISTITEM: i32 = 50007;
    const CT_LIST: i32 = 50008;
    const CT_MENU: i32 = 50009;
    const CT_MENUBAR: i32 = 50010;
    const CT_MENUITEM: i32 = 50011;
    const CT_RADIOBUTTON: i32 = 50013;
    const CT_SCROLLBAR: i32 = 50014;
    const CT_SLIDER: i32 = 50015;
    const CT_SPINNER: i32 = 50016;
    const CT_TAB: i32 = 50018;
    const CT_TABITEM: i32 = 50019;
    const CT_TEXT: i32 = 50020;
    const CT_TOOLBAR: i32 = 50021;
    const CT_TREE: i32 = 50023;
    const CT_TREEITEM: i32 = 50024;
    const CT_GROUP: i32 = 50026;
    const CT_DATAITEM: i32 = 50029;
    const CT_SPLITBUTTON: i32 = 50031;
    const CT_WINDOW: i32 = 50032;
    const CT_PANE: i32 = 50033;

    /// Map a UIA `ControlType` id onto the closest macOS `"AX*"` role string.
    ///
    /// The clickable control types (`Button`, `Edit`, `CheckBox`, …) are mapped
    /// onto the exact role strings in
    /// `builtin_tools/desktop/interactable.rs::INTERACTABLE_ROLES`, so they are
    /// picked up by `desktop_som` / `desktop_ax_snapshot` as actionable
    /// elements. Containers and decorative types map onto their
    /// non-interactable AX equivalents (deliberately *not* in the allowlist) so
    /// they show up in a full tree dump but never get a clickable mark. Unknown
    /// ids fall back to `"AXUnknown"`.
    pub const fn control_type_to_ax_role(control_type: i32) -> &'static str {
        match control_type {
            // ── interactable (must match INTERACTABLE_ROLES) ──
            CT_BUTTON => "AXButton",
            CT_SPLITBUTTON => "AXMenuButton",
            CT_CHECKBOX => "AXCheckBox",
            // A UIA tab item is selected like a macOS radio button (AX models
            // tabs as a radio group), so it belongs in the actionable set.
            CT_RADIOBUTTON | CT_TABITEM => "AXRadioButton",
            CT_COMBOBOX => "AXComboBox",
            CT_EDIT => "AXTextField",
            CT_HYPERLINK => "AXLink",
            CT_MENUITEM => "AXMenuItem",
            CT_SLIDER => "AXSlider",
            CT_SPINNER => "AXIncrementor",
            // ── non-interactable containers / decoration ──
            CT_WINDOW => "AXWindow",
            CT_PANE | CT_GROUP => "AXGroup",
            CT_MENU => "AXMenu",
            CT_MENUBAR => "AXMenuBar",
            CT_TOOLBAR => "AXToolbar",
            CT_LIST => "AXList",
            CT_TREE => "AXOutline",
            CT_LISTITEM | CT_TREEITEM | CT_DATAITEM => "AXRow",
            CT_TAB => "AXTabGroup",
            CT_TEXT => "AXStaticText",
            CT_IMAGE => "AXImage",
            CT_SCROLLBAR => "AXScrollBar",
            _ => "AXUnknown",
        }
    }
}

/// AX roles (as mapped by [`control_type_to_ax_role`]) that take typed text and
/// are therefore the only ones the label heuristic in [`is_password_like`] is
/// allowed to judge.
///
/// Restricting it matters: `secure` is a **hard block** on `type_text` that
/// `force` cannot override, so a heuristic that fired on any element whose label
/// merely contains "password" would refuse legitimate typing next to a
/// "Show password" checkbox or inside a window titled "Password Manager".
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
const TEXT_ENTRY_ROLES: &[&str] = &["AXTextField", "AXComboBox"];

/// Substrings that mark a text entry as carrying a credential.
///
/// Mirrors the term list orca's Windows runtime uses, because the failure being
/// prevented is identical: some frameworks (Electron, Qt, custom-drawn editors)
/// never set the UIA `IsPassword` property on a field that is nonetheless
/// masked, and typing a credential into the wrong place is not recoverable.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
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
/// The second signal behind UIA's own `IsPassword` — pure, so the judgement is
/// unit-testable without a live desktop. `role` is the **mapped** AX role;
/// `labels` are the element's Name / `AutomationId` / `ClassName`, whichever the
/// provider filled in.
///
/// Deliberately mechanical: it matches fixed substrings on fixed fields and
/// reads nothing else about the element (R7/P8).
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
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

/// The AX action names a UIA element's available patterns can honour.
///
/// Every name returned here must be one [`ax_action_to_patterns`] accepts —
/// advertising an action the write path then rejects would be worse than
/// advertising none, because the model plans on it.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn actions_for(has_press_pattern: bool, has_expand_collapse: bool) -> Vec<String> {
    let mut out = Vec::new();
    if has_press_pattern {
        out.push("AXPress".to_string());
    }
    if has_expand_collapse {
        out.push("AXShowMenu".to_string());
    }
    out
}

// The locator ranker lives in `aleph_desktop::ax_rank`: it is the contract the
// macOS Swift helper and the Linux AT-SPI limb also implement, and three
// hand-copied versions of one ranking rule is how platforms start disagreeing
// about which element a locator meant.
pub use aleph_desktop::{rank_candidates, RankCandidate};
/// UIA control patterns the AX write path can invoke, in fallback order.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxPattern {
    Invoke,
    Toggle,
    SelectionItem,
    ExpandCollapse,
    Legacy,
}

/// Map a macOS-style AX action name onto an ordered UIA-pattern fallback chain.
/// Covers the actions the tool DESCRIPTION advertises; everything else is an
/// honest `NotImplemented` the model can read and recover from.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn ax_action_to_patterns(action: &str) -> Result<Vec<AxPattern>> {
    match action {
        "AXPress" | "AXConfirm" => Ok(vec![
            AxPattern::Invoke,
            AxPattern::Toggle,
            AxPattern::SelectionItem,
            AxPattern::Legacy,
        ]),
        "AXShowMenu" => Ok(vec![AxPattern::ExpandCollapse]),
        other => Err(DesktopError::NotImplemented(format!(
            "ax.perform_action:{other}"
        ))),
    }
}

/// Depth guard for `query_by_role` (which carries no explicit `max_depth`).
#[cfg_attr(not(windows), allow(dead_code))]
const ROLE_SCAN_DEPTH: u32 = 12;

/// Depth bound for locator resolution walks (`set_value` / `perform_action`).
#[cfg_attr(not(windows), allow(dead_code))]
const RESOLVE_DEPTH: u32 = 12;

/// Hard cap on the number of nodes any single walk will materialize. Bounds the
/// response size and protects against pathological UI trees (P7 defensive
/// design) — the same spirit as the macOS helper's depth limit.
#[cfg_attr(not(windows), allow(dead_code))]
const MAX_NODES: usize = 4_000;

// ── Capability ───────────────────────────────────────────────────────────────

/// `AccessibilityCapability` backed by Windows UI Automation.
///
/// Stateless by design — see the module docs for why no COM pointer is cached.
pub struct WindowsAccessibility;

impl WindowsAccessibility {
    /// Create a new Windows accessibility capability.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for WindowsAccessibility {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AccessibilityCapability for WindowsAccessibility {
    async fn query_focused(&self) -> Result<Option<AxElement>> {
        run_blocking(imp::query_focused).await
    }

    async fn query_tree(&self, params: QueryTreeParams) -> Result<Option<AxElement>> {
        let pid = params.pid;
        let depth = params.max_depth;
        run_blocking(move || imp::query_tree(pid, depth)).await
    }

    async fn query_by_role(&self, params: QueryByRoleParams) -> Result<Vec<AxElement>> {
        let pid = params.pid;
        let role = params.role;
        run_blocking(move || imp::query_by_role(&role, pid)).await
    }

    async fn set_value(&self, params: SetValueParams) -> Result<AxActionResult> {
        let (locator, value) = (params.locator, params.value);
        run_blocking(move || imp::set_value(locator, value)).await
    }

    async fn perform_action(&self, params: PerformActionParams) -> Result<AxActionResult> {
        let (locator, action) = (params.locator, params.action);
        run_blocking(move || imp::perform_action(locator, action)).await
    }
}

/// Run COM work on a blocking thread, flattening join failures into a
/// `DesktopError`. All UIA access funnels through here so the panic/poison
/// handling lives in exactly one place.
async fn run_blocking<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(res) => res,
        Err(e) => Err(DesktopError::PlatformError(format!(
            "UI Automation worker thread failed: {e}"
        ))),
    }
}

// ── Windows COM implementation ───────────────────────────────────────────────

#[cfg(windows)]
mod imp {
    use super::{
        ax_action_to_patterns, control_type_to_ax_role, AxPattern, MAX_NODES, RESOLVE_DEPTH,
        ROLE_SCAN_DEPTH,
    };
    // The locator ranker lives in `aleph_desktop::ax_rank`: it is the contract
    // the macOS Swift helper and the Linux AT-SPI limb also implement, and three
    // hand-copied versions of one ranking rule is how platforms start
    // disagreeing about which element a locator meant.
    use aleph_desktop::{rank_candidates, DesktopError, RankCandidate, Result};
    use aleph_protocol::desktop_bridge::methods::ax::{
        AxActionResult, AxElement, AxLocator, AxVerification,
    };
    use aleph_protocol::desktop_bridge::methods::screen::Region;
    use windows::core::BSTR;

    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationExpandCollapsePattern,
        IUIAutomationInvokePattern, IUIAutomationLegacyIAccessiblePattern,
        IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern, IUIAutomationTreeWalker,
        IUIAutomationValuePattern, UIA_ExpandCollapsePatternId, UIA_InvokePatternId,
        UIA_LegacyIAccessiblePatternId, UIA_SelectionItemPatternId, UIA_TogglePatternId,
        UIA_ValuePatternId,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    /// RAII COM apartment guard. `CoInitializeEx` may return `S_FALSE` if the
    /// thread was already initialized — harmless; we still balance with
    /// `CoUninitialize` on drop.
    struct ComGuard;

    impl ComGuard {
        fn new() -> Self {
            // SAFETY: documented COM init; ignoring the HRESULT is correct here
            // (S_OK and S_FALSE are both success states for our purposes).
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            Self
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            // SAFETY: balances the `CoInitializeEx` in `new`.
            unsafe { CoUninitialize() };
        }
    }

    /// Create a fresh UI Automation root object for this thread.
    fn automation() -> Result<IUIAutomation> {
        // SAFETY: standard COM instantiation of the in-process UIA server.
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| DesktopError::PlatformError(format!("CUIAutomation create failed: {e}")))
    }

    /// Resolve the top-level window to root a query at: the first visible
    /// top-level window of `pid`, or the foreground window when `pid` is `None`.
    /// When `pid` is given but no visible window exists for it, return
    /// `NotAvailable` rather than silently falling back to the foreground
    /// window — that would let a caller asking for accessibility reads on
    /// PID A be quietly served an unrelated, foreground application B.
    ///
    /// Window discovery goes through the shared enumeration in
    /// `aleph_desktop::win_window`, so a query rooted at a pid cannot land on a
    /// cloaked window (a suspended UWP app, or one parked on another virtual
    /// desktop) that this module used to accept because `IsWindowVisible` says
    /// `true` for those.
    fn resolve_root_hwnd(pid: Option<i32>) -> Result<HWND> {
        if let Some(pid) = pid {
            let raw = u32::try_from(pid)
                .ok()
                .and_then(aleph_desktop::win_window::top_window_for_pid)
                .ok_or_else(|| {
                    DesktopError::NotAvailable(format!(
                        "no visible top-level window for pid {pid} to root the accessibility query"
                    ))
                })?;
            return Ok(HWND(raw as usize as *mut core::ffi::c_void));
        }
        // SAFETY: documented Win32 call; the returned handle is validated below.
        let fg = unsafe { GetForegroundWindow() };
        if fg.0.is_null() {
            return Err(DesktopError::NotAvailable(
                "no foreground window to root the accessibility query".into(),
            ));
        }
        Ok(fg)
    }

    /// Convert a UIA bounding rectangle (physical screen pixels, top-left
    /// origin) into the shared [`Region`]. Physical pixels are what the Windows
    /// click/screenshot path already uses, so coordinates round-trip directly
    /// into `desktop_click` without rescaling.
    fn rect_to_region(r: RECT) -> Region {
        Region {
            x: r.left as f64,
            y: r.top as f64,
            width: (r.right - r.left) as f64,
            height: (r.bottom - r.top) as f64,
        }
    }

    /// Read one element's scalar fields into a childless [`AxElement`].
    ///
    /// # What is (and is not) read per node
    ///
    /// `secure` and `enabled` are filled here, for every node of every walk:
    /// both are plain property getters in the same cost class as the name and
    /// control type already being read, and both are consumed by the
    /// model-facing projections (`affordance_fields` renders a greyed-out
    /// control and marks a password box, and `safe_value` withholds a secure
    /// element's text).
    ///
    /// `settable` and `actions` are **not** — determining them means probing up
    /// to five control patterns, i.e. five cross-process COM round trips per
    /// node against a `MAX_NODES` budget of 4 000. They are filled by
    /// [`enrich_resolved`] for the one element a call actually resolved, which
    /// is where every consumer reads them.
    fn node_of(el: &IUIAutomationElement) -> AxElement {
        // SAFETY: read-only UIA control-type property getter.
        let role = unsafe {
            el.CurrentControlType()
                .map_or("AXUnknown", |ct| control_type_to_ax_role(ct.0))
        }
        .to_string();
        // SAFETY: read-only UIA name property getter.
        let title = unsafe { el.CurrentName() }
            .map(|b| b.to_string())
            .ok()
            .filter(|s| !s.is_empty());
        // SAFETY: read-only UIA bounding-rectangle property getter.
        let bounds = unsafe { el.CurrentBoundingRectangle() }
            .ok()
            .map(rect_to_region);
        // SAFETY: read-only UIA process-id property getter.
        let pid = unsafe { el.CurrentProcessId() }.unwrap_or(0);
        // SAFETY: read-only UIA enabled-state property getter. A provider that
        // does not answer leaves this `None` — "not told", never `false`.
        let enabled = unsafe { el.CurrentIsEnabled() }.ok().map(|b| b.as_bool());

        AxElement {
            secure: Some(is_secure_element(el, &role)),
            enabled,
            role,
            title,
            // Values are read on demand, never for every node of a walk.
            value: None,
            bounds,
            pid,
            // `settable` / `actions` / `url` stay `None` here — see the doc
            // comment. `None` is the wire's "not told"; reporting `Some(false)`
            // would be an outright lie.
            ..Default::default()
        }
    }

    /// Whether this element masks its content.
    ///
    /// UIA's own `IsPassword` is the primary signal; the label heuristic is the
    /// second, for the frameworks that never set it (see [`is_password_like`]).
    /// A provider that errors on the property is treated as **not** secure, and
    /// the heuristic still gets its say — failing the other way would mark every
    /// element on an uncooperative provider as a password box and take typing
    /// away entirely.
    fn is_secure_element(el: &IUIAutomationElement, role: &str) -> bool {
        // SAFETY: read-only UIA password-attribute property getter.
        if unsafe { el.CurrentIsPassword() }
            .map(|b| b.as_bool())
            .unwrap_or(false)
        {
            return true;
        }
        // SAFETY: read-only UIA label property getters.
        let (name, automation_id, class_name) = unsafe {
            (
                el.CurrentName().map(|b| b.to_string()).unwrap_or_default(),
                el.CurrentAutomationId()
                    .map(|b| b.to_string())
                    .unwrap_or_default(),
                el.CurrentClassName()
                    .map(|b| b.to_string())
                    .unwrap_or_default(),
            )
        };
        super::is_password_like(
            role,
            &[name.as_str(), automation_id.as_str(), class_name.as_str()],
        )
    }

    /// Fill the affordances that cost a pattern probe, for the single element a
    /// call resolved: `settable`, `actions`, and the element's text `value`.
    ///
    /// The value is read **only when the element is not secure**. That check is
    /// the Windows counterpart of the macOS helper scrubbing a secure field's
    /// value inside the Swift handler: a password must not cross the limb
    /// boundary at all, rather than crossing it and being redacted later by
    /// whichever projection happens to remember to.
    fn enrich_resolved(el: &IUIAutomationElement, node: &mut AxElement) {
        // SAFETY: pattern getter; an element without the pattern surfaces as Err.
        let value_pattern =
            unsafe { el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }.ok();

        node.settable = Some(value_pattern.as_ref().is_some_and(|vp| {
            // SAFETY: read-only property getter. A provider that will not answer
            // is treated as writable — `set_value` verifies by read-back anyway,
            // so a wrong "yes" costs one honest failure, a wrong "no" costs the
            // model the whole write path.
            !unsafe { vp.CurrentIsReadOnly() }
                .map(|b| b.as_bool())
                .unwrap_or(false)
        }));

        // SAFETY: pattern getters; unsupported patterns surface as Err.
        let (has_press, has_expand) = unsafe {
            (
                el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                    .is_ok()
                    || el
                        .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                        .is_ok()
                    || el
                        .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                            UIA_SelectionItemPatternId,
                        )
                        .is_ok()
                    || el
                        .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                            UIA_LegacyIAccessiblePatternId,
                        )
                        .is_ok(),
                el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                    UIA_ExpandCollapsePatternId,
                )
                .is_ok(),
            )
        };
        let actions = super::actions_for(has_press, has_expand);
        node.actions = (!actions.is_empty()).then_some(actions);

        if node.secure != Some(true) {
            node.value = value_of(el, value_pattern.as_ref());
        }
    }

    /// Read an element's textual value: `ValuePattern.CurrentValue`, falling back
    /// to `LegacyIAccessible.CurrentValue`. Empty strings normalize to `None`.
    ///
    /// Callers must have established that the element is not secure — see
    /// [`enrich_resolved`], the only caller.
    fn value_of(
        el: &IUIAutomationElement,
        value_pattern: Option<&IUIAutomationValuePattern>,
    ) -> Option<String> {
        // SAFETY: read-only UIA pattern getters; missing pattern surfaces as Err.
        unsafe {
            if let Some(vp) = value_pattern {
                if let Ok(v) = vp.CurrentValue() {
                    let s = v.to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
            if let Ok(lp) = el.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                UIA_LegacyIAccessiblePatternId,
            ) {
                if let Ok(v) = lp.CurrentValue() {
                    let s = v.to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
        }
        None
    }

    /// Flatten the control-view subtree into `(RankCandidate, element)` pairs,
    /// bounded by `depth_remaining` and the global `MAX_NODES` budget. Each
    /// element handle is cloned (refcount bump) so it outlives the walk and can
    /// be acted on once ranking picks it.
    fn collect_candidates(
        walker: &IUIAutomationTreeWalker,
        el: &IUIAutomationElement,
        depth_remaining: u32,
        count: &mut usize,
        out: &mut Vec<(RankCandidate, IUIAutomationElement)>,
    ) {
        let node = node_of(el);
        let center = node
            .bounds
            .as_ref()
            .map_or((0.0, 0.0), |b| (b.x + b.width / 2.0, b.y + b.height / 2.0));
        out.push((
            RankCandidate {
                role: node.role,
                title: node.title,
                center,
            },
            el.clone(),
        ));
        *count += 1;
        if depth_remaining == 0 || *count >= MAX_NODES {
            return;
        }
        // SAFETY: walker child/sibling traversal; windows-rs surfaces "no
        // element" as Err, terminating each loop naturally.
        let mut next = unsafe { walker.GetFirstChildElement(el) };
        while let Ok(child) = next {
            collect_candidates(walker, &child, depth_remaining - 1, count, out);
            if *count >= MAX_NODES {
                break;
            }
            // SAFETY: walker sibling traversal; terminates at end of child list.
            next = unsafe { walker.GetNextSiblingElement(&child) };
        }
    }

    /// Resolve an [`AxLocator`] to the best-matching live element plus a
    /// value-enriched summary. Returns `None` when nothing matches the role
    /// filter (caller converts that to an `Err` with a recovery hint).
    pub(super) fn resolve(
        uia: &IUIAutomation,
        loc: &AxLocator,
    ) -> Result<Option<(IUIAutomationElement, AxElement)>> {
        let hwnd = resolve_root_hwnd(loc.pid)?;
        // SAFETY: `hwnd` is a validated visible/foreground window handle.
        let root = unsafe { uia.ElementFromHandle(hwnd) }
            .map_err(|e| DesktopError::PlatformError(format!("ElementFromHandle failed: {e}")))?;
        // SAFETY: standard "what a user sees" control-view walker.
        let walker = unsafe { uia.ControlViewWalker() }
            .map_err(|e| DesktopError::PlatformError(format!("ControlViewWalker failed: {e}")))?;
        let mut cands: Vec<(RankCandidate, IUIAutomationElement)> = Vec::new();
        let mut count = 0usize;
        collect_candidates(&walker, &root, RESOLVE_DEPTH, &mut count, &mut cands);

        let summaries: Vec<RankCandidate> = cands.iter().map(|(c, _)| c.clone()).collect();
        let Some(idx) = rank_candidates(&summaries, loc) else {
            return Ok(None);
        };
        let (_, el) = &cands[idx];
        // Re-read the element rather than reusing the ranking candidate: the
        // walk kept only the fields ranking needed, and the resolved element is
        // exactly the one worth paying the pattern probes for.
        let mut summary = node_of(el);
        enrich_resolved(el, &mut summary);
        Ok(Some((el.clone(), summary)))
    }

    /// Write `value` into the located element's UIA ValuePattern and read it
    /// back for verification. Only returns `Ok` when a write actually occurred;
    /// "not located / not settable" is an `Err` (see the Err-vs-Ok contract).
    pub(super) fn set_value(loc: AxLocator, value: String) -> Result<AxActionResult> {
        let _com = ComGuard::new();
        let uia = automation()?;
        let (el, mut summary) = resolve(&uia, &loc)?.ok_or_else(|| {
            DesktopError::NotAvailable("no element matched role/title; try `ax_snapshot`".into())
        })?;
        // SAFETY: pattern getter; unsupported pattern surfaces as Err.
        let vp = unsafe { el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }
            .map_err(|_| {
                DesktopError::NotAvailable(
                    "element does not support a settable value; fall back to click + type_text"
                        .into(),
                )
            })?;
        // SAFETY: read-only property getter.
        if unsafe { vp.CurrentIsReadOnly() }
            .map(|b| b.as_bool())
            .unwrap_or(false)
        {
            return Err(DesktopError::NotAvailable(
                "element is read-only; fall back to click + type_text".into(),
            ));
        }
        // SAFETY: documented ValuePattern write.
        unsafe { vp.SetValue(&BSTR::from(value.as_str())) }.map_err(|e| {
            DesktopError::PlatformError(format!("ValuePattern.SetValue failed: {e}"))
        })?;
        // SAFETY: read-back for verification.
        let readback = unsafe { vp.CurrentValue() }
            .map(|b| b.to_string())
            .unwrap_or_default();
        // A verification read of a masked field is still the credential. The
        // *comparison* is safe to report; the bytes are not, so neither the
        // mismatch preview nor the element summary carries them.
        let secure = summary.secure == Some(true);
        let verification = if readback == value {
            AxVerification {
                state: "verified".into(),
                reason: None,
                actual_preview: None,
            }
        } else {
            AxVerification {
                state: "unverified".into(),
                reason: Some("value_mismatch".into()),
                actual_preview: (!secure).then(|| readback.chars().take(200).collect()),
            }
        };
        summary.value = (!secure).then(|| readback.chars().take(200).collect());
        Ok(AxActionResult {
            performed: true,
            path: "accessibility".into(),
            matched: Some(summary),
            verification: Some(verification),
        })
    }

    /// Try to invoke one UIA pattern on `el`. `Ok(true)` = performed;
    /// `Ok(false)` = element does not expose this pattern (caller tries the
    /// next); `Err` = the pattern's action call itself failed.
    fn try_pattern(el: &IUIAutomationElement, pattern: AxPattern) -> Result<bool> {
        // SAFETY: each arm gets a pattern (Err if unsupported → Ok(false)) then
        // issues its documented action call.
        unsafe {
            match pattern {
                AxPattern::Invoke => {
                    if let Ok(p) =
                        el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                    {
                        p.Invoke()
                            .map_err(|e| DesktopError::PlatformError(format!("Invoke: {e}")))?;
                        return Ok(true);
                    }
                }
                AxPattern::Toggle => {
                    if let Ok(p) =
                        el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                    {
                        p.Toggle()
                            .map_err(|e| DesktopError::PlatformError(format!("Toggle: {e}")))?;
                        return Ok(true);
                    }
                }
                AxPattern::SelectionItem => {
                    if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                        UIA_SelectionItemPatternId,
                    ) {
                        p.Select()
                            .map_err(|e| DesktopError::PlatformError(format!("Select: {e}")))?;
                        return Ok(true);
                    }
                }
                AxPattern::ExpandCollapse => {
                    if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                        UIA_ExpandCollapsePatternId,
                    ) {
                        p.Expand()
                            .map_err(|e| DesktopError::PlatformError(format!("Expand: {e}")))?;
                        return Ok(true);
                    }
                }
                AxPattern::Legacy => {
                    if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                        UIA_LegacyIAccessiblePatternId,
                    ) {
                        p.DoDefaultAction().map_err(|e| {
                            DesktopError::PlatformError(format!("DoDefaultAction: {e}"))
                        })?;
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    /// Perform a macOS-style AX action on the located element by trying its
    /// UIA-pattern fallback chain. Unknown action → `NotImplemented` (from
    /// `ax_action_to_patterns`); no pattern usable → `NotAvailable`.
    pub(super) fn perform_action(loc: AxLocator, action: String) -> Result<AxActionResult> {
        let patterns = ax_action_to_patterns(&action)?;
        let _com = ComGuard::new();
        let uia = automation()?;
        let (el, summary) = resolve(&uia, &loc)?.ok_or_else(|| {
            DesktopError::NotAvailable("no element matched role/title; try `ax_snapshot`".into())
        })?;
        for pattern in patterns {
            if try_pattern(&el, pattern)? {
                return Ok(AxActionResult {
                    performed: true,
                    path: "accessibility".into(),
                    matched: Some(summary),
                    verification: None,
                });
            }
        }
        Err(DesktopError::NotAvailable(
            "element exposes no actionable pattern; try click at its center".into(),
        ))
    }

    /// Depth-first walk rooted at `el`, bounded by `depth_remaining` and the
    /// global `MAX_NODES` budget. `count` is the running node tally shared
    /// across the whole walk.
    fn walk(
        walker: &IUIAutomationTreeWalker,
        el: &IUIAutomationElement,
        depth_remaining: u32,
        count: &mut usize,
    ) -> AxElement {
        let mut node = node_of(el);
        *count += 1;
        if depth_remaining == 0 || *count >= MAX_NODES {
            return node;
        }
        // SAFETY: walker child/sibling traversal. windows-rs surfaces the UIA
        // "no element" (S_OK + null) result as `Err`, so the `while let Ok`
        // loop terminates naturally at the end of each child list.
        let mut next = unsafe { walker.GetFirstChildElement(el) };
        while let Ok(child) = next {
            node.children
                .push(walk(walker, &child, depth_remaining - 1, count));
            if *count >= MAX_NODES {
                break;
            }
            // SAFETY: walker sibling traversal; terminates at end of child list.
            next = unsafe { walker.GetNextSiblingElement(&child) };
        }
        node
    }

    pub(super) fn query_focused() -> Result<Option<AxElement>> {
        let _com = ComGuard::new();
        let uia = automation()?;
        // SAFETY: documented UIA call; a missing focus surfaces as `Err`.
        unsafe { uia.GetFocusedElement() }.map_or(Ok(None), |el| {
            let mut node = node_of(&el);
            // The focused element is the one the type_text pre-flight gate reads
            // (`secure` / `settable` / `role`), so it is worth the pattern
            // probes — and it is the single most important place for `secure` to
            // be right, because that gate is what stops a synthetic keystroke
            // from landing in a password box.
            enrich_resolved(&el, &mut node);
            Ok(Some(node))
        })
    }

    pub(super) fn query_tree(pid: Option<i32>, max_depth: u32) -> Result<Option<AxElement>> {
        let _com = ComGuard::new();
        let uia = automation()?;
        let hwnd = resolve_root_hwnd(pid)?;
        // SAFETY: `hwnd` is a validated visible/foreground window handle.
        let root = unsafe { uia.ElementFromHandle(hwnd) }
            .map_err(|e| DesktopError::PlatformError(format!("ElementFromHandle failed: {e}")))?;
        // SAFETY: the control-view walker is the standard "what a user sees"
        // traversal, matching the macOS AX tree's actionable framing.
        let walker = unsafe { uia.ControlViewWalker() }
            .map_err(|e| DesktopError::PlatformError(format!("ControlViewWalker failed: {e}")))?;
        let mut count = 0usize;
        Ok(Some(walk(&walker, &root, max_depth, &mut count)))
    }

    pub(super) fn query_by_role(role: &str, pid: Option<i32>) -> Result<Vec<AxElement>> {
        // Build a bounded tree, then collect nodes whose mapped AX role matches.
        // Walking + filtering (rather than a UIA property condition) reuses the
        // exact same role mapping the rest of the system sees, so results are
        // consistent with `query_tree` / `desktop_som`.
        let Some(tree) = query_tree(pid, ROLE_SCAN_DEPTH)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        collect_role(&tree, role, &mut out);
        Ok(out)
    }

    /// Depth-first collection of every node whose role equals `role`. Matches
    /// are pushed as childless clones — the tool surface only needs the matched
    /// element's own role/title/bounds, not its subtree.
    fn collect_role(node: &AxElement, role: &str, out: &mut Vec<AxElement>) {
        if node.role == role {
            // Detach the node from its subtree; every other field, affordances
            // included, is carried over verbatim.
            out.push(AxElement {
                children: Vec::new(),
                ..node.clone()
            });
        }
        for child in &node.children {
            collect_role(child, role, out);
        }
    }
}

// ── Non-Windows stub ─────────────────────────────────────────────────────────
//
// The crate compiles on every host (its unit tests exercise the pure role
// mapping), but the `windows` crate is only available under `cfg(windows)`.
// Off-Windows the trait methods report the capability as unavailable — they are
// never reached in production, where `DesktopPlatform::ax()` is only wired for
// the Windows target.
#[cfg(not(windows))]
mod imp {
    use aleph_desktop::{DesktopError, Result};
    use aleph_protocol::desktop_bridge::methods::ax::AxElement;

    fn unavailable<T>() -> Result<T> {
        Err(DesktopError::NotAvailable(
            "UI Automation is only available on Windows".into(),
        ))
    }

    pub(super) fn query_focused() -> Result<Option<AxElement>> {
        unavailable()
    }
    pub(super) fn query_tree(_pid: Option<i32>, _max_depth: u32) -> Result<Option<AxElement>> {
        unavailable()
    }
    pub(super) fn query_by_role(_role: &str, _pid: Option<i32>) -> Result<Vec<AxElement>> {
        unavailable()
    }
    pub(super) fn set_value(
        _loc: aleph_protocol::desktop_bridge::methods::ax::AxLocator,
        _value: String,
    ) -> Result<aleph_protocol::desktop_bridge::methods::ax::AxActionResult> {
        unavailable()
    }
    pub(super) fn perform_action(
        _loc: aleph_protocol::desktop_bridge::methods::ax::AxLocator,
        _action: String,
    ) -> Result<aleph_protocol::desktop_bridge::methods::ax::AxActionResult> {
        unavailable()
    }
}

// ── Tests (pure mapping, host-runnable) ──────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // The exact role strings the shared consumer treats as clickable. Mirrors
    // `builtin_tools/desktop/interactable.rs::INTERACTABLE_ROLES`; if a future
    // edit there drops one of these, this test documents the coupling.
    const INTERACTABLE: &[&str] = &[
        "AXButton",
        "AXMenuButton",
        "AXMenuItem",
        "AXCheckBox",
        "AXRadioButton",
        "AXTextField",
        "AXComboBox",
        "AXLink",
        "AXSlider",
        "AXIncrementor",
    ];

    #[test]
    fn clickable_control_types_map_into_the_interactable_allowlist() {
        // Every actionable UIA control type must land on a role the SoM /
        // snapshot tools will actually mark — otherwise the backend would
        // "work" yet produce zero clickable marks.
        for ct in [
            50000, // Button
            50031, // SplitButton
            50002, // CheckBox
            50013, // RadioButton
            50019, // TabItem
            50003, // ComboBox
            50004, // Edit
            50005, // Hyperlink
            50011, // MenuItem
            50015, // Slider
            50016, // Spinner
        ] {
            let role = control_type_to_ax_role(ct);
            assert!(
                INTERACTABLE.contains(&role),
                "control type {ct} mapped to non-interactable role {role}"
            );
        }
    }

    #[test]
    fn containers_map_to_non_interactable_roles() {
        // Containers should be visible in a full tree but never marked.
        for ct in [
            50032, // Window
            50026, // Group
            50020, // Text
            50008, // List
        ] {
            let role = control_type_to_ax_role(ct);
            assert!(
                !INTERACTABLE.contains(&role),
                "container control type {ct} unexpectedly marked interactable as {role}"
            );
        }
    }

    #[test]
    fn unknown_control_type_falls_back() {
        assert_eq!(control_type_to_ax_role(999_999), "AXUnknown");
    }

    #[test]
    fn window_and_text_have_expected_roles() {
        assert_eq!(control_type_to_ax_role(50032), "AXWindow");
        assert_eq!(control_type_to_ax_role(50020), "AXStaticText");
        assert_eq!(control_type_to_ax_role(50004), "AXTextField");
    }

    #[test]
    fn axpress_maps_to_invoke_fallback_chain() {
        assert_eq!(
            ax_action_to_patterns("AXPress").unwrap(),
            vec![
                AxPattern::Invoke,
                AxPattern::Toggle,
                AxPattern::SelectionItem,
                AxPattern::Legacy
            ]
        );
    }

    #[test]
    fn axconfirm_same_as_axpress() {
        assert_eq!(
            ax_action_to_patterns("AXConfirm").unwrap(),
            ax_action_to_patterns("AXPress").unwrap()
        );
    }

    #[test]
    fn axshowmenu_maps_to_expand_collapse() {
        assert_eq!(
            ax_action_to_patterns("AXShowMenu").unwrap(),
            vec![AxPattern::ExpandCollapse]
        );
    }

    #[test]
    fn unknown_action_is_not_implemented() {
        let err = ax_action_to_patterns("AXFoo").unwrap_err();
        assert!(matches!(
            err,
            aleph_desktop::DesktopError::NotImplemented(_)
        ));
    }

    // ── Credential-field heuristic ───────────────────────────────────────────

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
    }

    #[test]
    fn the_heuristic_reads_automation_id_and_class_name_too() {
        // Electron and Qt often leave Name empty and put the meaning elsewhere.
        assert!(is_password_like("AXTextField", &["", "login-password", ""]));
        assert!(is_password_like("AXTextField", &["", "", "PasswordBox"]));
    }

    #[test]
    fn only_text_entry_roles_are_judged_by_label() {
        // The blast radius matters: `secure` is a hard block that `force` cannot
        // lift, so a checkbox labelled "Show password" or a group inside a
        // password manager must not silently disable typing everywhere.
        for role in ["AXCheckBox", "AXButton", "AXGroup", "AXStaticText"] {
            assert!(
                !is_password_like(role, &["Show password", "", ""]),
                "{role} must not be judged by its label"
            );
        }
    }

    #[test]
    fn pin_matches_only_as_a_whole_word() {
        assert!(is_password_like("AXTextField", &["Enter PIN", "", ""]));
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
        assert!(!is_password_like("AXTextField", &["", "", ""]));
    }

    // ── Advertised actions ───────────────────────────────────────────────────

    #[test]
    fn advertised_actions_are_all_accepted_by_the_write_path() {
        // Advertising an action `perform_action` then rejects is worse than
        // advertising none: the model plans on it.
        for actions in [
            actions_for(true, true),
            actions_for(true, false),
            actions_for(false, true),
        ] {
            for action in &actions {
                assert!(
                    ax_action_to_patterns(action).is_ok(),
                    "{action} is advertised but not accepted"
                );
            }
        }
    }

    #[test]
    fn an_element_with_no_patterns_advertises_nothing() {
        assert!(actions_for(false, false).is_empty());
    }
}
