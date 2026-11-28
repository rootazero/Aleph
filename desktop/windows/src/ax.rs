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
//! UIA work runs on one MTA worker. Its client and retained elements live and
//! drop on that thread. Callers exchange owned requests and Rust value results.

use async_trait::async_trait;

use aleph_desktop::traits::AccessibilityCapability;
use aleph_desktop::{DesktopError, Result};
use aleph_protocol::desktop_bridge::methods::ax::{
    AxActionResult, AxElement, PerformActionParams, QueryByRoleParams, QueryFocusedParams,
    QueryListResult, QueryResult, QueryTreeParams, SetValueParams,
};

#[cfg(windows)]
use aleph_protocol::desktop_bridge::methods::ax::{
    clamp_max_nodes, AxReferenceAction, AxReferenceSnapshot, AxSnapshotRequest,
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

// The credential-label heuristic that backs `secure` lives in
// `aleph_desktop::ax_secure` — the Linux AT-SPI limb has the same blind spot on
// the same frameworks (Electron / Qt editors that never set the native
// "this is a password" signal), and it cannot reach into this crate. Imported
// under its own name so `is_secure_element` below reads unchanged.
#[cfg(windows)]
use aleph_desktop::is_password_like;

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

// The locator ranker lives in `aleph_desktop::ax_rank` — see the import inside
// `imp`, its only consumer. (A second, module-level re-export of it lived here
// and was never named by anything.)

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

/// Wall-clock ceiling on one accessibility walk.
///
/// The node budget bounds how *much* a walk reads; it does not bound how *long*
/// a single read takes. Every UI Automation property getter is a cross-process
/// call into the target application's UI thread, and an application that is not
/// pumping messages — mid-freeze, showing a modal on another thread, or being
/// debugged — answers each one only when its per-call COM timeout expires. A
/// full budget of nodes against such a provider blocks the worker thread far
/// past anything the caller can use, so the walk stops and returns the subtree
/// it did get. A partial tree the model can act on beats a stalled turn.
#[cfg_attr(not(windows), allow(dead_code))]
const WALK_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

// ── Capability ───────────────────────────────────────────────────────────────

/// `AccessibilityCapability` backed by Windows UI Automation.
pub struct WindowsAccessibility {
    #[cfg(windows)]
    worker: std::sync::OnceLock<std::result::Result<imp::WorkerClient, String>>,
}

impl WindowsAccessibility {
    /// Create a new Windows accessibility capability.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            #[cfg(windows)]
            worker: std::sync::OnceLock::new(),
        }
    }

    #[cfg(windows)]
    async fn run<T, F>(&self, command: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut imp::WorkerState, u64) -> Result<T> + Send + 'static,
    {
        self.worker
            .get_or_init(imp::WorkerClient::connect)
            .as_ref()
            .map_err(|message| DesktopError::PlatformError(message.clone()))?
            .run(command)
            .await
    }
}

impl Default for WindowsAccessibility {
    fn default() -> Self {
        Self::new()
    }
}

// Keep the non-Windows stub at the dispatch boundary.
macro_rules! uia_call {
    ($capability:expr, $params:ident, move |$state:ident, _| $command:expr) => {{
        #[cfg(windows)]
        {
            $capability.run(move |$state, _| $command).await
        }
        #[cfg(not(windows))]
        {
            let _ = $params;
            Err(DesktopError::NotAvailable(
                "UI Automation is only available on Windows".into(),
            ))
        }
    }};
}

#[async_trait]
impl AccessibilityCapability for WindowsAccessibility {
    async fn query_focused(&self, params: QueryFocusedParams) -> Result<Option<AxElement>> {
        uia_call!(self, params, move |state, _| {
            Ok(imp::query_focused(&state.uia)?.filter(|el| params.pid.is_none_or(|p| el.pid == p)))
        })
    }

    async fn query_tree(&self, params: QueryTreeParams) -> Result<QueryResult> {
        uia_call!(self, params, move |state, _| imp::query_tree(
            &state.uia,
            params.pid,
            params.max_depth,
            clamp_max_nodes(params.max_nodes),
        ))
    }

    async fn query_by_role(&self, params: QueryByRoleParams) -> Result<QueryListResult> {
        uia_call!(self, params, move |state, _| imp::query_by_role(
            &state.uia,
            &params.role,
            params.pid,
            clamp_max_nodes(params.max_nodes),
        ))
    }

    async fn set_value(&self, params: SetValueParams) -> Result<AxActionResult> {
        uia_call!(self, params, move |state, _| imp::set_value(
            &state.uia,
            &params.locator,
            &params.value
        ))
    }

    async fn perform_action(&self, params: PerformActionParams) -> Result<AxActionResult> {
        uia_call!(self, params, move |state, _| imp::perform_action(
            &state.uia,
            &params.locator,
            &params.action
        ))
    }

    #[cfg(windows)]
    async fn snapshot_references(&self, request: AxSnapshotRequest) -> Result<AxReferenceSnapshot> {
        self.run(move |state, owner| state.snapshot_references(owner, request))
            .await
    }

    #[cfg(windows)]
    async fn act_on_reference(&self, request: AxReferenceAction) -> Result<AxActionResult> {
        self.run(move |state, owner| state.act_on_reference(owner, request))
            .await
    }
}

// ── Windows COM implementation ───────────────────────────────────────────────

#[cfg(windows)]
mod imp {
    use super::{
        ax_action_to_patterns, control_type_to_ax_role, AxPattern, RESOLVE_DEPTH, ROLE_SCAN_DEPTH,
        WALK_BUDGET,
    };
    use std::collections::VecDeque;
    use std::hash::{BuildHasher, RandomState};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{mpsc, Arc, Mutex, Weak};
    use std::time::{Duration, Instant};
    use tokio::sync::oneshot;
    // The locator ranker lives in `aleph_desktop::ax_rank`: it is the contract
    // the macOS Swift helper and the Linux AT-SPI limb also implement, and three
    // hand-copied versions of one ranking rule is how platforms start
    // disagreeing about which element a locator meant.
    use aleph_desktop::{rank_candidates, DesktopError, RankCandidate, Result};
    use aleph_protocol::desktop_bridge::methods::ax::{
        clamp_max_nodes, AxActionResult, AxElement, AxLocator, AxMutation, AxReferenceAction,
        AxReferenceError, AxReferenceSnapshot, AxSnapshotRequest, AxSnapshotRow, AxVerification,
        NativeAxReference, QueryListResult, QueryResult, AX_REFERENCE_TTL_SECS, DEFAULT_MAX_NODES,
        MAX_AX_REFERENCE_ROWS, MAX_AX_REFERENCE_SNAPSHOTS,
    };
    use aleph_protocol::desktop_bridge::methods::screen::Region;
    use windows::core::{BOOL, BSTR};

    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED, SAFEARRAY,
    };
    use windows::Win32::System::Ole::{
        SafeArrayDestroy, SafeArrayGetDim, SafeArrayGetElement, SafeArrayGetElemsize,
        SafeArrayGetLBound, SafeArrayGetUBound,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, ExpandCollapseState_Expanded, ExpandCollapseState_LeafNode, IUIAutomation,
        IUIAutomationCacheRequest, IUIAutomationElement, IUIAutomationExpandCollapsePattern,
        IUIAutomationInvokePattern, IUIAutomationLegacyIAccessiblePattern,
        IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern, IUIAutomationTreeWalker,
        IUIAutomationValuePattern, UIA_AutomationIdPropertyId, UIA_BoundingRectanglePropertyId,
        UIA_ClassNamePropertyId, UIA_ControlTypePropertyId, UIA_ExpandCollapsePatternId,
        UIA_InvokePatternId, UIA_IsEnabledPropertyId, UIA_IsOffscreenPropertyId,
        UIA_IsPasswordPropertyId, UIA_LegacyIAccessiblePatternId, UIA_NamePropertyId,
        UIA_NativeWindowHandlePropertyId, UIA_ProcessIdPropertyId, UIA_SelectionItemPatternId,
        UIA_TogglePatternId, UIA_ValuePatternId,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetForegroundWindow, GetWindowThreadProcessId, GA_ROOT,
    };

    const WORKER_QUEUE_CAPACITY: usize = 32;
    const SNAPSHOT_MAX_DEPTH: u32 = 32;
    const REFERENCE_TTL: Duration = Duration::from_secs(AX_REFERENCE_TTL_SECS);

    type Command = Box<dyn FnOnce(&mut WorkerState) + Send>;
    type CommandSender = mpsc::SyncSender<Command>;

    pub(super) struct WorkerClient {
        sender: Arc<CommandSender>,
        owner: u64,
    }

    impl WorkerClient {
        pub(super) fn connect() -> std::result::Result<Self, String> {
            static WORKER: Mutex<Weak<CommandSender>> = Mutex::new(Weak::new());
            static NEXT_OWNER: AtomicU64 = AtomicU64::new(0);

            let mut worker = WORKER
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let sender = if let Some(sender) = worker.upgrade() {
                sender
            } else {
                let (sender, receiver) = mpsc::sync_channel(WORKER_QUEUE_CAPACITY);
                std::thread::Builder::new()
                    .name("aleph-uia".into())
                    .spawn(move || {
                        if let Err(error) = run_worker(receiver) {
                            tracing::debug!(%error, "UI Automation worker failed");
                        }
                    })
                    .map_err(|e| format!("UI Automation worker: {e}"))?;
                let sender = Arc::new(sender);
                *worker = Arc::downgrade(&sender);
                sender
            };
            Ok(Self {
                sender,
                owner: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
            })
        }

        pub(super) async fn run<T, F>(&self, command: F) -> Result<T>
        where
            T: Send + 'static,
            F: FnOnce(&mut WorkerState, u64) -> Result<T> + Send + 'static,
        {
            let (reply, receiver) = oneshot::channel();
            let owner = self.owner;
            self.sender
                .try_send(Box::new(move |state| {
                    if !reply.is_closed() {
                        // Once started, finish the native work even if the receiver goes away.
                        let result = command(state, owner);
                        let _ = reply.send(result);
                    }
                }))
                .map_err(|error| match error {
                    mpsc::TrySendError::Full(_) => reference_error(
                        AxReferenceError::ActionPending,
                        "The UI Automation worker is busy. Wait for current work to settle.",
                    ),
                    mpsc::TrySendError::Disconnected(_) => worker_unavailable(),
                })?;
            receiver.await.map_err(|_| worker_unavailable())?
        }
    }

    fn worker_unavailable() -> DesktopError {
        DesktopError::PlatformError("The UI Automation worker is unavailable".into())
    }

    fn run_worker(receiver: mpsc::Receiver<Command>) -> Result<()> {
        // A previous owner can still be finishing a native call after its last
        // sender dropped. Dispose of that client before constructing another.
        static APARTMENT: Mutex<()> = Mutex::new(());
        let _serial = APARTMENT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _com = ComGuard::new()?;
        // SAFETY: standard COM instantiation, on this thread's MTA.
        let uia = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.map_err(
            |e| DesktopError::PlatformError(format!("CUIAutomation create failed: {e}")),
        )?;
        let mut state = WorkerState {
            leases: VecDeque::new(),
            nonce: RandomState::new().hash_one(0u8),
            next_lease: 0,
            uia,
        };
        run_commands(&receiver, &mut state);
        Ok(())
    }

    fn run_commands(receiver: &mpsc::Receiver<Command>, state: &mut WorkerState) {
        loop {
            let now = Instant::now();
            state.expire(now);
            let command = if let Some(lease) = state.leases.front() {
                receiver.recv_timeout(lease.expires.saturating_duration_since(now))
            } else {
                receiver
                    .recv()
                    .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
            };
            match command {
                Ok(command) => {
                    state.expire(Instant::now());
                    command(state);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    struct ComGuard;

    impl ComGuard {
        fn new() -> Result<Self> {
            // SAFETY: this thread starts without a COM apartment.
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
                .ok()
                .map_err(|e| DesktopError::PlatformError(format!("CoInitializeEx(MTA): {e}")))?;
            Ok(Self)
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            // SAFETY: balances this thread's successful CoInitializeEx, after
            // all UIA objects on the worker have been released.
            unsafe { CoUninitialize() };
        }
    }

    #[cfg(test)]
    mod worker_tests {
        use super::*;
        use std::thread;

        pub(super) struct ExpiryProbe(mpsc::Sender<thread::ThreadId>);

        impl Drop for ExpiryProbe {
            fn drop(&mut self) {
                let _ = self.0.send(thread::current().id());
            }
        }

        #[tokio::test]
        async fn uia_worker_expires_retained_objects_on_the_owning_thread_while_idle() {
            let worker = WorkerClient::connect().expect("start UIA worker");
            let (dropped, wait) = mpsc::channel();
            let owner_thread = worker
                .run(move |state, owner| {
                    // SAFETY: this command runs on the owning MTA.
                    let root = unsafe { state.uia.GetRootElement() }.expect("desktop UIA root");
                    let identity = ProviderIdentity::capture(&state.uia, &root)
                        .expect("desktop UIA root identity");
                    state.leases.push_front(ReferenceLease {
                        id: "expiry-probe".into(),
                        owner,
                        expires: Instant::now() + Duration::from_millis(25),
                        scope: WindowScope {
                            hwnd: HWND::default(),
                            pid: 0,
                            thread: 0,
                        },
                        root,
                        root_identity: identity,
                        elements: Vec::new(),
                        _expiry_probe: Some(ExpiryProbe(dropped)),
                    });
                    Ok(thread::current().id())
                })
                .await
                .expect("insert lease on the worker");
            assert_ne!(thread::current().id(), owner_thread);
            assert_eq!(
                wait.recv_timeout(Duration::from_secs(3))
                    .expect("idle expiry"),
                owner_thread
            );
            // Holding the client until after the assertion excludes shutdown cleanup.
            drop(worker);
        }
        #[tokio::test]
        async fn uia_worker_finishes_started_command_before_readback_after_cancellation() {
            let worker = Arc::new(WorkerClient::connect().expect("start UIA worker"));
            let (started, wait_for_start) = oneshot::channel();
            let (finish, wait_for_finish) = mpsc::channel();
            let settled = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let command = tokio::spawn({
                let worker = Arc::clone(&worker);
                let settled = Arc::clone(&settled);
                async move {
                    worker
                        .run(move |_, _| {
                            let _ = started.send(());
                            wait_for_finish.recv().expect("allow command to settle");
                            settled.store(true, Ordering::Release);
                            Ok(())
                        })
                        .await
                }
            });
            wait_for_start.await.expect("command started");
            command.abort();
            assert!(command.await.expect_err("caller cancelled").is_cancelled());
            let mut readback =
                Box::pin(worker.run(move |_, _| Ok(settled.load(Ordering::Acquire))));
            let early_readback =
                tokio::time::timeout(Duration::from_millis(25), &mut readback).await;
            finish.send(()).expect("allow command to settle");
            assert!(
                early_readback.is_err(),
                "readback overtook a started command"
            );
            assert!(readback.await.expect("read after the command settled"));
        }
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
            // The HWND is a pointer-sized handle but the window APIs we
            // consulted all return it as `u32`; saturate on platforms where
            // `usize` is narrower so the truncation is loud, not silent.
            return Ok(HWND(usize::try_from(raw).map_err(|_| {
                DesktopError::PlatformError(format!(
                    "top_window_for_pid returned 0x{raw:x}, does not fit in this platform's usize"
                ))
            })? as *mut core::ffi::c_void));
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

    fn reference_error(code: AxReferenceError, message: &'static str) -> DesktopError {
        DesktopError::AxReference {
            code,
            message: message.into(),
        }
    }

    fn stale_reference() -> DesktopError {
        reference_error(
            AxReferenceError::StaleReference,
            "The retained UI Automation target is no longer valid. Take a fresh snapshot.",
        )
    }

    fn unsupported_window() -> DesktopError {
        reference_error(
            AxReferenceError::WindowScopeUnsupported,
            "The window is not a live UI Automation root for the requested process.",
        )
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    struct WindowScope {
        hwnd: HWND,
        pid: i32,
        thread: u32,
        launch_stamp: u64,
    }

    impl WindowScope {
        fn from_hwnd(hwnd: HWND) -> Option<Self> {
            let mut pid = 0;
            // SAFETY: Win32 validates HWNDs; pid points to writable storage.
            let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            if thread == 0 || pid == 0 || unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd {
                return None;
            }
            let pid = i32::try_from(pid).ok()?;
            let identity = aleph_desktop::win_window::desktop_process_identity(pid)?;
            Some(Self {
                hwnd,
                pid,
                thread,
                launch_stamp: identity.launch_stamp,
            })
        }

        fn root(self, uia: &IUIAutomation) -> Option<IUIAutomationElement> {
            if Self::from_hwnd(self.hwnd) != Some(self) {
                return None;
            }
            // SAFETY: read-only getters on the object mapped to the exact HWND.
            let root = unsafe { uia.ElementFromHandle(self.hwnd) }.ok()?;
            (unsafe { root.CurrentProcessId() }.ok()? == self.pid
                && unsafe { root.CurrentNativeWindowHandle() }.ok()? == self.hwnd)
                .then_some(root)
        }

        fn contains_hwnd(self, hwnd: HWND) -> bool {
            let mut pid = 0;
            // SAFETY: Win32 validates HWNDs; pid points to writable storage.
            unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut pid)) != 0
                    && i32::try_from(pid) == Ok(self.pid)
                    && GetAncestor(hwnd, GA_ROOT) == self.hwnd
            }
        }

        fn contains(self, raw: &IUIAutomationTreeWalker, el: &IUIAutomationElement) -> bool {
            // SAFETY: read-only access and ancestry traversal on the owning MTA.
            unsafe {
                if el.CurrentProcessId().ok() != Some(self.pid) {
                    return false;
                }
                let mut current = el.clone();
                for _ in 0..=SNAPSHOT_MAX_DEPTH {
                    let Ok(hwnd) = current.CurrentNativeWindowHandle() else {
                        return false;
                    };
                    if !hwnd.0.is_null() {
                        return self.contains_hwnd(hwnd);
                    }
                    let Ok(parent) = raw.GetParentElement(&current) else {
                        return false;
                    };
                    current = parent;
                }
                false
            }
        }
    }

    struct RuntimeIdArray(*mut SAFEARRAY);

    impl Drop for RuntimeIdArray {
        fn drop(&mut self) {
            // SAFETY: GetRuntimeId transferred ownership of this non-null array.
            let _ = unsafe { SafeArrayDestroy(self.0) };
        }
    }

    fn runtime_id(el: &IUIAutomationElement) -> Option<Vec<i32>> {
        // SAFETY: GetRuntimeId returns a caller-owned SAFEARRAY of integers.
        let ptr = unsafe { el.GetRuntimeId() }.ok()?;
        if ptr.is_null() {
            return None;
        }
        let array = RuntimeIdArray(ptr);
        // SAFETY: array is live and owned here. UIA returns one dimension of
        // 32-bit integers. Verify the shape before copying into i32 storage.
        unsafe {
            if SafeArrayGetDim(array.0) != 1
                || SafeArrayGetElemsize(array.0) as usize != std::mem::size_of::<i32>()
            {
                return None;
            }
            let lower = SafeArrayGetLBound(array.0, 1).ok()?;
            let upper = SafeArrayGetUBound(array.0, 1).ok()?;
            let count = upper.checked_sub(lower)?.checked_add(1)?;
            if !(1..=128).contains(&count) {
                return None;
            }
            let mut id = Vec::with_capacity(count as usize);
            for index in lower..=upper {
                let mut part = 0i32;
                SafeArrayGetElement(array.0, &index, (&mut part as *mut i32).cast()).ok()?;
                id.push(part);
            }
            Some(id)
        }
    }

    enum ProviderIdentity {
        RuntimeId(Vec<i32>),
        Hwnd(HWND),
    }

    impl ProviderIdentity {
        fn capture(uia: &IUIAutomation, el: &IUIAutomationElement) -> Option<Self> {
            if let Some(id) = runtime_id(el) {
                return Some(Self::RuntimeId(id));
            }
            // An HWND is usable as provider evidence only if UIA maps it back
            // to the same element. Associated ancestor handles are insufficient.
            let hwnd = unsafe { el.CurrentNativeWindowHandle() }.ok()?;
            let identity = Self::Hwnd(hwnd);
            identity.matches(uia, el).then_some(identity)
        }

        fn matches(&self, uia: &IUIAutomation, el: &IUIAutomationElement) -> bool {
            match self {
                Self::RuntimeId(expected) => runtime_id(el).as_ref() == Some(expected),
                Self::Hwnd(hwnd) => {
                    // SAFETY: read-only current identity checks on the same MTA.
                    unsafe {
                        !hwnd.0.is_null()
                            && el.CurrentNativeWindowHandle().ok() == Some(*hwnd)
                            && uia.ElementFromHandle(*hwnd).is_ok_and(|current| {
                                uia.CompareElements(el, &current).is_ok_and(BOOL::as_bool)
                            })
                    }
                }
            }
        }
    }

    struct RetainedElement {
        element: IUIAutomationElement,
        identity: ProviderIdentity,
        control_type: i32,
        name: Option<String>,
        settable: bool,
        actions: Vec<String>,
    }

    struct ReferenceLease {
        id: String,
        owner: u64,
        expires: Instant,
        scope: WindowScope,
        root: IUIAutomationElement,
        root_identity: ProviderIdentity,
        elements: Vec<RetainedElement>,
        #[cfg(test)]
        _expiry_probe: Option<worker_tests::ExpiryProbe>,
    }

    pub(super) struct WorkerState {
        // Fields are dropped in declaration order, before the COM guard.
        leases: VecDeque<ReferenceLease>,
        nonce: u64,
        next_lease: u64,
        pub(super) uia: IUIAutomation,
    }

    impl WorkerState {
        fn expire(&mut self, now: Instant) {
            while self
                .leases
                .front()
                .is_some_and(|lease| lease.expires <= now)
            {
                self.leases.pop_front();
            }
        }

        pub(super) fn snapshot_references(
            &mut self,
            owner: u64,
            request: AxSnapshotRequest,
        ) -> Result<AxReferenceSnapshot> {
            let hwnd = match request.window_id {
                Some(id) => HWND(usize::try_from(id).map_err(|_| unsupported_window())?
                    as *mut core::ffi::c_void),
                None => resolve_root_hwnd(request.pid)?,
            };
            // Validate native ownership before any UIA calls for this snapshot.
            let scope = WindowScope::from_hwnd(hwnd)
                .filter(|scope| request.pid.is_none_or(|pid| pid == scope.pid))
                .ok_or_else(unsupported_window)?;
            let root = scope.root(&self.uia).ok_or_else(unsupported_window)?;
            let root_identity = ProviderIdentity::capture(&self.uia, &root);
            let cache = cache_request(&self.uia)?;
            // SAFETY: build a cache on the selected root and create read-only walkers.
            let cached_root = unsafe { root.BuildUpdatedCache(&cache) }.map_err(|e| {
                DesktopError::PlatformError(format!("BuildUpdatedCache failed: {e}"))
            })?;
            let walker = unsafe { self.uia.ControlViewWalker() }.map_err(|e| {
                DesktopError::PlatformError(format!("ControlViewWalker failed: {e}"))
            })?;
            let raw = unsafe { self.uia.RawViewWalker() }
                .map_err(|e| DesktopError::PlatformError(format!("RawViewWalker failed: {e}")))?;
            let id = format!("{:016x}-{:016x}", self.nonce, self.next_lease);
            self.next_lease += 1;
            let budget = Budget::new(clamp_max_nodes(request.max_nodes));
            let mut snapshot = SnapshotWalk {
                uia: &self.uia,
                walker: &walker,
                raw: &raw,
                cache: &cache,
                scope,
                budget: &budget,
                id: root_identity.as_ref().map(|_| id.as_str()),
                row_limit: request.max_elements.min(MAX_AX_REFERENCE_ROWS),
                rows: Vec::new(),
                retained: Vec::new(),
                total_rows: 0,
                total_interactable: 0,
            };
            snapshot.visit(&cached_root, request.max_depth.min(SNAPSHOT_MAX_DEPTH));
            let focused = if budget.exhausted() {
                None
            } else {
                focused_element(&self.uia, &cache, |el| scope.contains(&raw, el), true)
            };
            if WindowScope::from_hwnd(hwnd) != Some(scope) {
                return Err(unsupported_window());
            }
            let result = AxReferenceSnapshot {
                app_pid: scope.pid,
                window_id: Some(scope.hwnd.0 as usize as u64),
                tree_available: true,
                elements: snapshot.rows,
                focused,
                tree_truncated: budget.exhausted(),
                truncated: snapshot.total_rows > snapshot.row_limit,
                nodes_walked: budget.spent(),
                total_interactable: snapshot.total_interactable,
            };
            let retained = snapshot.retained;
            if let Some(root_identity) = root_identity.filter(|_| !retained.is_empty()) {
                let now = Instant::now();
                self.expire(now);
                if self.leases.len() == MAX_AX_REFERENCE_SNAPSHOTS {
                    self.leases.pop_front();
                }
                self.leases.push_back(ReferenceLease {
                    id,
                    owner,
                    expires: now + REFERENCE_TTL,
                    scope,
                    root,
                    root_identity,
                    elements: retained,
                    #[cfg(test)]
                    _expiry_probe: None,
                });
            }
            Ok(result)
        }

        pub(super) fn act_on_reference(
            &self,
            owner: u64,
            request: AxReferenceAction,
        ) -> Result<AxActionResult> {
            let lease = self
                .leases
                .iter()
                .find(|lease| lease.owner == owner && lease.id == request.reference.lease)
                .ok_or_else(stale_reference)?;
            let retained = lease
                .elements
                .get(request.reference.index as usize)
                .ok_or_else(stale_reference)?;
            let root = lease.scope.root(&self.uia).ok_or_else(stale_reference)?;
            // SAFETY: read-only comparisons and provider checks on the owning MTA.
            let raw = unsafe { self.uia.RawViewWalker() }.map_err(|_| stale_reference())?;
            let el = &retained.element;
            let root_matches = lease.root_identity.matches(&self.uia, &root)
                && unsafe { self.uia.CompareElements(&lease.root, &root) }.is_ok_and(BOOL::as_bool);
            if !root_matches
                || !lease.scope.contains(&raw, el)
                || !retained.identity.matches(&self.uia, el)
            {
                return Err(stale_reference());
            }
            let control_type = unsafe { el.CurrentControlType() }
                .map_err(|_| stale_reference())?
                .0;
            let title = nonempty(
                unsafe { el.CurrentName() }
                    .map_err(|_| stale_reference())?
                    .to_string(),
            );
            if control_type != retained.control_type || title != retained.name {
                return Err(stale_reference());
            }
            check_enabled(el)?;
            let mut summary = node_of(el);
            summary.enabled = Some(true);
            summary.secure = Some(summary.secure == Some(true) || current_password_or_unknown(el));
            let target = Target::Reference(lease.expires);
            match request.mutation {
                AxMutation::SetValue { value } if retained.settable => {
                    summary.settable = Some(true);
                    set_value_on(el, summary, &value, target)
                }
                AxMutation::Perform { action } if retained.actions.contains(&action) => {
                    let patterns =
                        ax_action_to_patterns(&action).map_err(|_| unsupported_action())?;
                    perform_action_on(el, summary, patterns, target)
                }
                _ => Err(unsupported_action()),
            }
        }
    }

    struct SnapshotWalk<'a> {
        uia: &'a IUIAutomation,
        walker: &'a IUIAutomationTreeWalker,
        raw: &'a IUIAutomationTreeWalker,
        cache: &'a IUIAutomationCacheRequest,
        budget: &'a Budget,
        scope: WindowScope,
        id: Option<&'a str>,
        row_limit: usize,
        rows: Vec<AxSnapshotRow>,
        retained: Vec<RetainedElement>,
        total_rows: usize,
        total_interactable: usize,
    }

    impl SnapshotWalk<'_> {
        fn visit(&mut self, el: &IUIAutomationElement, depth: u32) {
            let exhausted = self.budget.spend();
            // SAFETY: cached read with live fallback. Native child windows
            // must belong to the captured top-level window.
            let hwnd = unsafe {
                el.CachedNativeWindowHandle()
                    .or_else(|_| el.CurrentNativeWindowHandle())
            };
            if !hwnd.is_ok_and(|hwnd| hwnd.0.is_null() || self.scope.contains_hwnd(hwnd)) {
                return;
            }
            let mut node = node_of(el);
            if node.pid == self.scope.pid {
                self.total_rows += 1;
                let publish = self.rows.len() < self.row_limit;
                if publish {
                    node.secure =
                        Some(node.secure == Some(true) || current_password_or_unknown(el));
                }
                enrich(el, &mut node, true, publish);
                let actionable = node.enabled == Some(true)
                    && (node.settable == Some(true) || node.actions.is_some());
                self.total_interactable += usize::from(actionable);
                if publish {
                    let reference = self
                        .id
                        .filter(|_| actionable && self.scope.contains(self.raw, el))
                        .and_then(|id| self.retain(el, &node, id));
                    self.rows.push(AxSnapshotRow {
                        element: node,
                        reference,
                    });
                }
            }
            if !exhausted && depth > 0 && !self.budget.exhausted() {
                walk_children(self.walker, self.cache, el, self.budget, |child| {
                    self.visit(child, depth - 1);
                });
            }
        }

        fn retain(
            &mut self,
            el: &IUIAutomationElement,
            node: &AxElement,
            id: &str,
        ) -> Option<NativeAxReference> {
            let identity = ProviderIdentity::capture(self.uia, el)?;
            let control_type = control_type_of(el);
            if control_type == 0 {
                return None;
            }
            let index = self.retained.len() as u32;
            self.retained.push(RetainedElement {
                element: el.clone(),
                identity,
                control_type,
                name: node.title.clone(),
                settable: node.settable == Some(true),
                actions: node.actions.clone().unwrap_or_default(),
            });
            Some(NativeAxReference {
                lease: id.into(),
                index,
            })
        }
    }

    /// Convert a UIA bounding rectangle (physical screen pixels, top-left
    /// origin) into the shared [`Region`]. Physical pixels are what the Windows
    /// click/screenshot path already uses, so coordinates round-trip directly
    /// into `desktop_click` without rescaling.
    fn rect_to_region(r: RECT) -> Region {
        Region {
            x: f64::from(r.left),
            y: f64::from(r.top),
            width: f64::from(r.right - r.left),
            height: f64::from(r.bottom - r.top),
        }
    }

    /// The property set every node read needs, requested once so the provider
    /// answers them all in a single cross-process round trip.
    ///
    /// Without it, [`node_of`] costs eight separate calls into the target
    /// application's UI thread — times a `MAX_NODES` budget of 4 000, that is
    /// over thirty thousand round trips for one `desktop_som`. UI Automation's
    /// answer to exactly this is the cache request: `*BuildCache` navigation
    /// returns each element with its properties already attached, so the reads
    /// afterwards are in-process.
    ///
    /// The request deliberately leaves `AutomationElementMode` at its default
    /// (`Full`), which keeps the live provider reference on every returned
    /// element. That is what lets [`node_of`] fall back to a `Current*` getter
    /// when a provider declines to populate the cache, and what lets the pattern
    /// probes in [`enrich_resolved`] work on a cached element at all.
    fn cache_request(uia: &IUIAutomation) -> Result<IUIAutomationCacheRequest> {
        // SAFETY: documented factory call on the automation root.
        let req = unsafe { uia.CreateCacheRequest() }
            .map_err(|e| DesktopError::PlatformError(format!("CreateCacheRequest failed: {e}")))?;
        for id in [
            UIA_ControlTypePropertyId,
            UIA_NamePropertyId,
            UIA_BoundingRectanglePropertyId,
            UIA_ProcessIdPropertyId,
            UIA_NativeWindowHandlePropertyId,
            UIA_IsEnabledPropertyId,
            UIA_IsPasswordPropertyId,
            UIA_IsOffscreenPropertyId,
            UIA_AutomationIdPropertyId,
            UIA_ClassNamePropertyId,
        ] {
            // SAFETY: documented cache-request builder; a property the platform
            // does not know is refused here rather than mid-walk, and the
            // `Current*` fallback covers it.
            let _ = unsafe { req.AddProperty(id) };
        }
        Ok(req)
    }

    /// Read one element's scalar fields into a childless [`AxElement`].
    ///
    /// # What is (and is not) read per node
    ///
    /// `secure`, `enabled` and the offscreen flag are filled here, for every
    /// node of every walk: they arrive with the cached property set (see
    /// [`cache_request`]) at no extra round trip, and all three are consumed by
    /// the model-facing projections (`affordance_fields` renders a greyed-out
    /// control and marks a password box, and `safe_value` withholds a secure
    /// element's text).
    ///
    /// `settable` and `actions` are **not** — determining them means probing up
    /// to five control patterns, which the cache cannot carry and which are five
    /// genuine round trips per node. They are filled by [`enrich_resolved`] for
    /// the one element a call actually resolved, which is where every consumer
    /// reads them.
    ///
    /// # Offscreen elements report no bounds
    ///
    /// UI Automation keeps scrolled-out list items, collapsed menu entries and
    /// every element of a minimized window in the tree, with `IsOffscreen` set
    /// and a rectangle that is stale (or the (-32000, -32000) parking sentinel).
    /// Passing that rectangle on would put a Set-of-Marks label at a spot the
    /// element is not at — and whatever *is* there gets clicked instead. So an
    /// offscreen element keeps its role and title (a locator can still address
    /// it; `set_value` through UI Automation does not need visibility) and loses
    /// its bounds, which `usable_bounds` then filters out of the marks.
    fn node_of(el: &IUIAutomationElement) -> AxElement {
        let role = control_type_to_ax_role(control_type_of(el)).to_string();
        let title = nonempty(name_of(el));
        let bounds = if is_offscreen(el) {
            None
        } else {
            // SAFETY: cached read with a live-getter fallback; both are
            // read-only UIA bounding-rectangle getters.
            unsafe {
                el.CachedBoundingRectangle()
                    .or_else(|_| el.CurrentBoundingRectangle())
            }
            .ok()
            .map(rect_to_region)
        };
        // SAFETY: cached read with a live-getter fallback.
        let pid = unsafe { el.CachedProcessId().or_else(|_| el.CurrentProcessId()) }.unwrap_or(0);
        // SAFETY: cached read with a live-getter fallback. A provider that does
        // not answer either leaves this `None` — "not told", never `false`.
        let enabled = unsafe { el.CachedIsEnabled().or_else(|_| el.CurrentIsEnabled()) }
            .ok()
            .map(BOOL::as_bool);

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

    /// The element's UIA `ControlType` id, cached read first.
    fn control_type_of(el: &IUIAutomationElement) -> i32 {
        // SAFETY: cached read with a live-getter fallback.
        unsafe { el.CachedControlType().or_else(|_| el.CurrentControlType()) }.map_or(0, |ct| ct.0)
    }

    fn nonempty(text: String) -> Option<String> {
        (!text.is_empty()).then_some(text)
    }

    fn current_password_or_unknown(el: &IUIAutomationElement) -> bool {
        // SAFETY: read-only property. Missing data cannot establish a safe value read.
        unsafe { el.CurrentIsPassword() }.map_or(true, BOOL::as_bool)
    }

    /// The element's `Name`, empty when unknown.
    fn name_of(el: &IUIAutomationElement) -> String {
        // SAFETY: cached read with a live-getter fallback.
        unsafe { el.CachedName().or_else(|_| el.CurrentName()) }
            .map(|b| b.to_string())
            .unwrap_or_default()
    }

    /// Whether UI Automation says this element is not currently on screen.
    ///
    /// A provider that will not answer is treated as **on** screen: an element
    /// wrongly hidden from the marks is a capability silently lost, whereas a
    /// stale rectangle is at worst one misplaced mark the model can see is wrong.
    fn is_offscreen(el: &IUIAutomationElement) -> bool {
        // SAFETY: cached read with a live-getter fallback.
        unsafe { el.CachedIsOffscreen().or_else(|_| el.CurrentIsOffscreen()) }
            .is_ok_and(BOOL::as_bool)
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
        // SAFETY: cached read with a live-getter fallback.
        if unsafe { el.CachedIsPassword().or_else(|_| el.CurrentIsPassword()) }
            .is_ok_and(BOOL::as_bool)
        {
            return true;
        }
        // SAFETY: cached reads with live-getter fallbacks; all read-only.
        let (automation_id, class_name) = unsafe {
            (
                el.CachedAutomationId()
                    .or_else(|_| el.CurrentAutomationId())
                    .map(|b| b.to_string())
                    .unwrap_or_default(),
                el.CachedClassName()
                    .or_else(|_| el.CurrentClassName())
                    .map(|b| b.to_string())
                    .unwrap_or_default(),
            )
        };
        let name = name_of(el);
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
        enrich(el, node, false, true);
    }

    fn enrich(el: &IUIAutomationElement, node: &mut AxElement, strict: bool, read_value: bool) {
        // SAFETY: pattern getter; an element without the pattern surfaces as Err.
        let value_pattern =
            unsafe { el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }.ok();

        node.settable = Some(
            value_pattern
                .as_ref()
                .is_some_and(|vp| !read_only(vp, strict)),
        );

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
                        .is_ok_and(|pattern| legacy_available(&pattern, strict)),
                el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                    UIA_ExpandCollapsePatternId,
                )
                .is_ok_and(|pattern| {
                    !strict
                        || pattern.CurrentExpandCollapseState().is_ok_and(|state| {
                            state != ExpandCollapseState_LeafNode
                                && state != ExpandCollapseState_Expanded
                        })
                }),
            )
        };
        let actions = super::actions_for(has_press, has_expand);
        node.actions = (!actions.is_empty()).then_some(actions);

        if read_value && node.secure != Some(true) {
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

    /// The two things that stop a walk: how many nodes are left, and how much
    /// wall clock is left.
    ///
    /// Both were previously spelled out at four call sites as a `&mut usize`
    /// compared against `MAX_NODES`; folding them into one value is what makes
    /// adding the deadline a change in one place rather than four, and keeps
    /// "have we run out?" a single question every walker asks the same way.
    struct Budget {
        allowance: usize,
        remaining: std::cell::Cell<usize>,
        deadline: Instant,
    }

    impl Budget {
        /// `max_nodes` comes from the protocol (already clamped by the caller);
        /// it used to be a private `MAX_NODES = 4_000` here, one of three
        /// different per-platform answers to the same question.
        fn new(max_nodes: u32) -> Self {
            let allowance = max_nodes as usize;
            Self {
                allowance,
                remaining: std::cell::Cell::new(allowance),
                deadline: Instant::now() + WALK_BUDGET,
            }
        }

        /// Charge one node and report whether the walk must stop now.
        fn spend(&self) -> bool {
            self.remaining.set(self.remaining.get().saturating_sub(1));
            self.exhausted()
        }

        fn exhausted(&self) -> bool {
            self.remaining.get() == 0 || Instant::now() >= self.deadline
        }

        /// Nodes charged so far.
        fn spent(&self) -> u32 {
            u32::try_from(self.allowance.saturating_sub(self.remaining.get())).unwrap_or(u32::MAX)
        }
    }

    /// Flatten the control-view subtree into `(RankCandidate, element)` pairs,
    /// bounded by [`Budget`]. Each element handle is cloned (refcount bump) so
    /// it outlives the walk and can be acted on once ranking picks it.
    fn collect_candidates(
        walker: &IUIAutomationTreeWalker,
        cache: &IUIAutomationCacheRequest,
        el: &IUIAutomationElement,
        depth_remaining: u32,
        budget: &Budget,
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
        if budget.spend() || depth_remaining == 0 {
            return;
        }
        // SAFETY: walker child/sibling traversal; windows-rs surfaces "no
        // element" as Err, terminating each loop naturally.
        let mut next = unsafe { walker.GetFirstChildElementBuildCache(el, cache) };
        while let Ok(child) = next {
            collect_candidates(walker, cache, &child, depth_remaining - 1, budget, out);
            if budget.exhausted() {
                break;
            }
            // SAFETY: walker sibling traversal; terminates at end of child list.
            next = unsafe { walker.GetNextSiblingElementBuildCache(&child, cache) };
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
        let cache = cache_request(uia)?;
        // SAFETY: `hwnd` is a validated visible/foreground window handle.
        let root = unsafe { uia.ElementFromHandleBuildCache(hwnd, &cache) }.map_err(|e| {
            DesktopError::PlatformError(format!("ElementFromHandleBuildCache failed: {e}"))
        })?;
        // SAFETY: standard "what a user sees" control-view walker.
        let walker = unsafe { uia.ControlViewWalker() }
            .map_err(|e| DesktopError::PlatformError(format!("ControlViewWalker failed: {e}")))?;
        let mut cands: Vec<(RankCandidate, IUIAutomationElement)> = Vec::new();
        let budget = Budget::new(DEFAULT_MAX_NODES);
        collect_candidates(&walker, &cache, &root, RESOLVE_DEPTH, &budget, &mut cands);

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

    #[derive(Clone, Copy)]
    enum Target {
        Locator,
        Reference(Instant),
    }

    impl Target {
        fn is_reference(self) -> bool {
            matches!(self, Self::Reference(_))
        }

        fn unsupported(self, locator_message: &'static str) -> DesktopError {
            match self {
                Self::Locator => DesktopError::NotAvailable(locator_message.into()),
                Self::Reference(_) => unsupported_action(),
            }
        }

        fn dispatch_error(self, method: &'static str, error: windows::core::Error) -> DesktopError {
            match self {
                Self::Locator => DesktopError::PlatformError(format!("{method}: {error}")),
                Self::Reference(_) => reference_error(
                    AxReferenceError::ActionOutcomeUnknown,
                    "UI Automation did not confirm the action outcome. Observe the target again.",
                ),
            }
        }

        fn check_dispatch(self, el: &IUIAutomationElement) -> Result<()> {
            if let Self::Reference(expires) = self {
                check_enabled(el)?;
                if Instant::now() >= expires {
                    return Err(stale_reference());
                }
            }
            Ok(())
        }
    }

    fn unsupported_action() -> DesktopError {
        reference_error(
            AxReferenceError::ActionUnsupported,
            "The retained element does not support the requested mutation.",
        )
    }

    fn check_enabled(el: &IUIAutomationElement) -> Result<()> {
        // SAFETY: read-only current provider state on the owning MTA.
        if unsafe { el.CurrentIsEnabled() }
            .map_err(|_| stale_reference())?
            .as_bool()
        {
            Ok(())
        } else {
            Err(reference_error(
                AxReferenceError::ElementDisabled,
                "The element is disabled.",
            ))
        }
    }

    fn read_only(pattern: &IUIAutomationValuePattern, strict: bool) -> bool {
        // SAFETY: read-only UIA property getter.
        unsafe { pattern.CurrentIsReadOnly() }.map_or(strict, BOOL::as_bool)
    }

    fn legacy_available(pattern: &IUIAutomationLegacyIAccessiblePattern, strict: bool) -> bool {
        // SAFETY: read-only UIA property getter.
        !strict || unsafe { pattern.CurrentDefaultAction() }.is_ok_and(|action| !action.is_empty())
    }

    /// Write `value` into the located element's UIA ValuePattern and read it back.
    pub(super) fn set_value(
        uia: &IUIAutomation,
        loc: &AxLocator,
        value: &str,
    ) -> Result<AxActionResult> {
        let (el, summary) = resolve(uia, loc)?.ok_or_else(|| {
            DesktopError::NotAvailable("no element matched role/title; try `ax_snapshot`".into())
        })?;
        set_value_on(&el, summary, value, Target::Locator)
    }

    fn set_value_on(
        el: &IUIAutomationElement,
        mut summary: AxElement,
        value: &str,
        target: Target,
    ) -> Result<AxActionResult> {
        // SAFETY: pattern getter; unsupported pattern surfaces as Err.
        let vp = unsafe { el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }
            .map_err(|_| {
                target.unsupported(
                    "element does not support a settable value; fall back to click + type_text",
                )
            })?;
        if read_only(&vp, target.is_reference()) {
            return Err(target.unsupported("element is read-only; fall back to click + type_text"));
        }
        target.check_dispatch(el)?;
        // SAFETY: documented ValuePattern write, on the validated target.
        unsafe { vp.SetValue(&BSTR::from(value)) }
            .map_err(|e| target.dispatch_error("ValuePattern.SetValue failed", e))?;
        // SAFETY: read-back for verification.
        let readback = unsafe { vp.CurrentValue() }.map(|b| b.to_string());
        let verified = match &readback {
            Ok(actual) => actual == value,
            Err(_) => !target.is_reference() && value.is_empty(),
        };
        let readback = readback.unwrap_or_default();
        // A verification read of a masked field is still the credential. The
        // *comparison* is safe to report; the bytes are not, so neither the
        // mismatch preview nor the element summary carries them.
        let secure = summary.secure == Some(true);
        let verification = if verified {
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
    fn try_pattern(el: &IUIAutomationElement, pattern: AxPattern, target: Target) -> Result<bool> {
        // SAFETY: each arm gets a pattern (Err if unsupported → Ok(false)) then
        // issues its documented action call.
        unsafe {
            match pattern {
                AxPattern::Invoke => {
                    if let Ok(p) =
                        el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                    {
                        target.check_dispatch(el)?;
                        p.Invoke().map_err(|e| target.dispatch_error("Invoke", e))?;
                        return Ok(true);
                    }
                }
                AxPattern::Toggle => {
                    if let Ok(p) =
                        el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                    {
                        target.check_dispatch(el)?;
                        p.Toggle().map_err(|e| target.dispatch_error("Toggle", e))?;
                        return Ok(true);
                    }
                }
                AxPattern::SelectionItem => {
                    if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                        UIA_SelectionItemPatternId,
                    ) {
                        target.check_dispatch(el)?;
                        p.Select().map_err(|e| target.dispatch_error("Select", e))?;
                        return Ok(true);
                    }
                }
                AxPattern::ExpandCollapse => {
                    if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                        UIA_ExpandCollapsePatternId,
                    ) {
                        match p.CurrentExpandCollapseState() {
                            // Already showing. `Expand()` on an expanded element
                            // is an error on several providers, which turned
                            // "the menu you asked for is open" into a hard
                            // failure the model had to recover from.
                            Ok(state) if state == ExpandCollapseState_Expanded => {
                                target.check_dispatch(el)?;
                                return Ok(!target.is_reference());
                            }
                            // A leaf has nothing to show; say so by falling
                            // through to the next pattern (there is none for
                            // `AXShowMenu`, so the caller reports it honestly)
                            // rather than claiming a menu appeared.
                            Ok(state) if state == ExpandCollapseState_LeafNode => return Ok(false),
                            Err(_) if target.is_reference() => return Ok(false),
                            _ => {}
                        }
                        target.check_dispatch(el)?;
                        p.Expand().map_err(|e| target.dispatch_error("Expand", e))?;
                        return Ok(true);
                    }
                }
                AxPattern::Legacy => {
                    if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                        UIA_LegacyIAccessiblePatternId,
                    ) {
                        if legacy_available(&p, target.is_reference()) {
                            target.check_dispatch(el)?;
                            p.DoDefaultAction()
                                .map_err(|e| target.dispatch_error("DoDefaultAction", e))?;
                            return Ok(true);
                        }
                    }
                }
            }
        }
        Ok(false)
    }

    /// Perform a macOS-style AX action on the located element by trying its
    /// UIA-pattern fallback chain. Unknown action → `NotImplemented` (from
    /// `ax_action_to_patterns`); no pattern usable → `NotAvailable`.
    pub(super) fn perform_action(
        uia: &IUIAutomation,
        loc: &AxLocator,
        action: &str,
    ) -> Result<AxActionResult> {
        let patterns = ax_action_to_patterns(action)?;
        let (el, summary) = resolve(uia, loc)?.ok_or_else(|| {
            DesktopError::NotAvailable("no element matched role/title; try `ax_snapshot`".into())
        })?;
        perform_action_on(&el, summary, patterns, Target::Locator)
    }

    fn perform_action_on(
        el: &IUIAutomationElement,
        summary: AxElement,
        patterns: Vec<AxPattern>,
        target: Target,
    ) -> Result<AxActionResult> {
        for pattern in patterns {
            if try_pattern(el, pattern, target)? {
                return Ok(AxActionResult {
                    performed: true,
                    path: "accessibility".into(),
                    matched: Some(summary),
                    verification: None,
                });
            }
        }
        Err(target.unsupported("element exposes no actionable pattern; try click at its center"))
    }

    /// Depth-first walk rooted at `el`, bounded by `depth_remaining` and the
    /// shared [`Budget`] (node count *and* wall clock).
    fn walk(
        walker: &IUIAutomationTreeWalker,
        cache: &IUIAutomationCacheRequest,
        el: &IUIAutomationElement,
        depth_remaining: u32,
        budget: &Budget,
    ) -> AxElement {
        let mut node = node_of(el);
        if budget.spend() || depth_remaining == 0 {
            return node;
        }
        walk_children(walker, cache, el, budget, |child| {
            node.children
                .push(walk(walker, cache, child, depth_remaining - 1, budget));
        });
        node
    }

    fn walk_children(
        walker: &IUIAutomationTreeWalker,
        cache: &IUIAutomationCacheRequest,
        el: &IUIAutomationElement,
        budget: &Budget,
        mut visit: impl FnMut(&IUIAutomationElement),
    ) {
        // SAFETY: read-only control-view traversal. A null result terminates traversal.
        let mut next = unsafe { walker.GetFirstChildElementBuildCache(el, cache) };
        while let Ok(child) = next {
            visit(&child);
            if budget.exhausted() {
                break;
            }
            next = unsafe { walker.GetNextSiblingElementBuildCache(&child, cache) };
        }
    }

    fn focused_element(
        uia: &IUIAutomation,
        cache: &IUIAutomationCacheRequest,
        in_scope: impl FnOnce(&IUIAutomationElement) -> bool,
        strict: bool,
    ) -> Option<AxElement> {
        // SAFETY: read-only UIA call; a missing focus surfaces as Err.
        let el = unsafe { uia.GetFocusedElementBuildCache(cache) }
            .ok()
            .filter(in_scope)?;
        let mut node = node_of(&el);
        if strict {
            node.secure = Some(node.secure == Some(true) || current_password_or_unknown(&el));
        }
        enrich(&el, &mut node, strict, true);
        Some(node)
    }

    pub(super) fn query_focused(uia: &IUIAutomation) -> Result<Option<AxElement>> {
        let cache = cache_request(uia)?;
        Ok(focused_element(uia, &cache, |_| true, false))
    }

    pub(super) fn query_tree(
        uia: &IUIAutomation,
        pid: Option<i32>,
        max_depth: u32,
        max_nodes: u32,
    ) -> Result<QueryResult> {
        let hwnd = resolve_root_hwnd(pid)?;
        let cache = cache_request(uia)?;
        // SAFETY: `hwnd` is a validated visible/foreground window handle.
        let root = unsafe { uia.ElementFromHandleBuildCache(hwnd, &cache) }.map_err(|e| {
            DesktopError::PlatformError(format!("ElementFromHandleBuildCache failed: {e}"))
        })?;
        // SAFETY: the control-view walker is the standard "what a user sees"
        // traversal, matching the macOS AX tree's actionable framing.
        let walker = unsafe { uia.ControlViewWalker() }
            .map_err(|e| DesktopError::PlatformError(format!("ControlViewWalker failed: {e}")))?;
        let budget = Budget::new(max_nodes);
        let element = walk(&walker, &cache, &root, max_depth, &budget);
        Ok(QueryResult {
            element: Some(element),
            node_count: budget.spent(),
            // Either bound ending the walk means the same thing to the caller:
            // what came back is not all of it.
            truncated: budget.exhausted(),
        })
    }

    pub(super) fn query_by_role(
        uia: &IUIAutomation,
        role: &str,
        pid: Option<i32>,
        max_nodes: u32,
    ) -> Result<QueryListResult> {
        // Build a bounded tree, then collect nodes whose mapped AX role matches.
        // Walking + filtering (rather than a UIA property condition) reuses the
        // exact same role mapping the rest of the system sees, so results are
        // consistent with `query_tree` / `desktop_som`.
        let tree = query_tree(uia, pid, ROLE_SCAN_DEPTH, max_nodes)?;
        let mut out = Vec::new();
        if let Some(root) = tree.element.as_ref() {
            collect_role(root, role, &mut out);
        }
        Ok(QueryListResult {
            elements: out,
            node_count: tree.node_count,
            truncated: tree.truncated,
        })
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

    // The credential-label heuristic and its tests live in
    // `aleph_desktop::ax_secure`, which both this limb and the Linux AT-SPI one
    // consume — the tests moved with it rather than being duplicated here.

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
