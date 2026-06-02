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
    AxElement, QueryByRoleParams, QueryTreeParams,
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
    pub fn control_type_to_ax_role(control_type: i32) -> &'static str {
        match control_type {
            // ── interactable (must match INTERACTABLE_ROLES) ──
            CT_BUTTON => "AXButton",
            CT_SPLITBUTTON => "AXMenuButton",
            CT_CHECKBOX => "AXCheckBox",
            CT_RADIOBUTTON => "AXRadioButton",
            // A UIA tab item is selected like a macOS radio button (AX models
            // tabs as a radio group), so it belongs in the actionable set.
            CT_TABITEM => "AXRadioButton",
            CT_COMBOBOX => "AXComboBox",
            CT_EDIT => "AXTextField",
            CT_HYPERLINK => "AXLink",
            CT_MENUITEM => "AXMenuItem",
            CT_SLIDER => "AXSlider",
            CT_SPINNER => "AXIncrementor",
            // ── non-interactable containers / decoration ──
            CT_WINDOW => "AXWindow",
            CT_PANE => "AXGroup",
            CT_GROUP => "AXGroup",
            CT_MENU => "AXMenu",
            CT_MENUBAR => "AXMenuBar",
            CT_TOOLBAR => "AXToolbar",
            CT_LIST => "AXList",
            CT_LISTITEM => "AXRow",
            CT_TREE => "AXOutline",
            CT_TREEITEM => "AXRow",
            CT_DATAITEM => "AXRow",
            CT_TAB => "AXTabGroup",
            CT_TEXT => "AXStaticText",
            CT_IMAGE => "AXImage",
            CT_SCROLLBAR => "AXScrollBar",
            _ => "AXUnknown",
        }
    }
}

/// Depth guard for `query_by_role` (which carries no explicit `max_depth`).
#[cfg_attr(not(windows), allow(dead_code))]
const ROLE_SCAN_DEPTH: u32 = 12;

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
    pub fn new() -> Self {
        WindowsAccessibility
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
    use super::{control_type_to_ax_role, MAX_NODES, ROLE_SCAN_DEPTH};
    use aleph_desktop::{DesktopError, Result};
    use aleph_protocol::desktop_bridge::methods::ax::AxElement;
    use aleph_protocol::desktop_bridge::methods::screen::Region;

    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTreeWalker,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible,
    };

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
            ComGuard
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
    /// top-level window of `pid`, or the foreground window when `pid` is `None`
    /// (or no window of `pid` is visible).
    fn resolve_root_hwnd(pid: Option<i32>) -> Result<HWND> {
        if let Some(pid) = pid {
            if let Some(hwnd) = top_window_for_pid(pid as u32) {
                return Ok(hwnd);
            }
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

    /// Find the first visible top-level window owned by `pid`.
    fn top_window_for_pid(pid: u32) -> Option<HWND> {
        struct Find {
            pid: u32,
            hit: Option<HWND>,
        }

        extern "system" fn proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
            // SAFETY: `lparam` carries the `&mut Find` we pass to `EnumWindows`,
            // which outlives this synchronous enumeration.
            unsafe {
                if IsWindowVisible(hwnd).as_bool() {
                    let mut wpid: u32 = 0;
                    GetWindowThreadProcessId(hwnd, Some(&mut wpid));
                    let find = &mut *(lparam.0 as *mut Find);
                    if wpid == find.pid {
                        find.hit = Some(hwnd);
                        return BOOL(0); // stop — first match wins
                    }
                }
            }
            BOOL(1)
        }

        let mut find = Find { pid, hit: None };
        // SAFETY: `proc` matches `WNDENUMPROC`; `find` lives until `EnumWindows`
        // returns.
        unsafe {
            let _ = EnumWindows(Some(proc), LPARAM(&mut find as *mut Find as isize));
        }
        find.hit
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
    fn node_of(el: &IUIAutomationElement) -> AxElement {
        // SAFETY: all four are documented read-only UIA property getters.
        let role = unsafe {
            el.CurrentControlType()
                .map(|ct| control_type_to_ax_role(ct.0))
                .unwrap_or("AXUnknown")
        }
        .to_string();
        let title = unsafe { el.CurrentName() }
            .map(|b| b.to_string())
            .ok()
            .filter(|s| !s.is_empty());
        let bounds = unsafe { el.CurrentBoundingRectangle() }.ok().map(rect_to_region);
        let pid = unsafe { el.CurrentProcessId() }.unwrap_or(0);

        AxElement {
            role,
            title,
            value: None, // VARIANT value extraction deferred; Name covers labels.
            bounds,
            pid,
            children: Vec::new(),
        }
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
            next = unsafe { walker.GetNextSiblingElement(&child) };
        }
        node
    }

    pub(super) fn query_focused() -> Result<Option<AxElement>> {
        let _com = ComGuard::new();
        let uia = automation()?;
        // SAFETY: documented UIA call; a missing focus surfaces as `Err`.
        match unsafe { uia.GetFocusedElement() } {
            Ok(el) => Ok(Some(node_of(&el))),
            Err(_) => Ok(None),
        }
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
        let tree = match query_tree(pid, ROLE_SCAN_DEPTH)? {
            Some(t) => t,
            None => return Ok(Vec::new()),
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
            out.push(AxElement {
                role: node.role.clone(),
                title: node.title.clone(),
                value: node.value.clone(),
                bounds: node.bounds.clone(),
                pid: node.pid,
                children: Vec::new(),
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
}
