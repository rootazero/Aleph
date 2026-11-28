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
//! 2. **Locators and snapshot references have distinct lifetimes.** Locators
//!    still use [`aleph_desktop::rank_candidates`]. A reference leases one
//!    bounded AT-SPI address on the issuing connection.
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

mod budget;
mod bus;
mod cache;
mod roles;
mod walk;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use atspi::State;
use atspi::proxy::accessible::AccessibleProxy;
use tokio::task::AbortHandle;
use tokio::time::Instant;

use aleph_desktop::traits::AccessibilityCapability;
use aleph_desktop::{DesktopError, RankCandidate, Result, rank_candidates};
use aleph_protocol::desktop_bridge::methods::ax::{
    AX_REFERENCE_TTL_SECS, AxActionResult, AxElement, AxLocator, AxMutation, AxReferenceAction,
    AxReferenceError, AxReferenceSnapshot, AxSnapshotRequest, AxSnapshotRow, AxVerification,
    MAX_AX_REFERENCE_SNAPSHOTS, NativeAxReference, PerformActionParams, QueryByRoleParams,
    QueryFocusedParams, QueryListResult, QueryResult, QueryTreeParams, SetValueParams,
    clamp_max_nodes,
};

pub use bus::{bus_looks_reachable, frontmost_pid};

use budget::Budget;
use bus::{App, Bus};
use cache::AppCache;
use walk::{ReferenceTarget, Walk};

/// Depth bound for the walks that carry no explicit `max_depth`
/// (`query_by_role`, locator resolution). Deep enough to reach a control inside
/// a few layers of layout containers, bounded enough not to descend a web
/// document forever.
const SCAN_DEPTH: u32 = 12;

/// Depth bound for the focused-element search.
const FOCUS_DEPTH: u32 = 24;

/// `AccessibilityCapability` backed by AT-SPI2 over D-Bus.
pub struct LinuxAccessibility {
    references: Arc<Mutex<ReferenceLeases>>,
}

impl LinuxAccessibility {
    /// Build the capability if an accessibility bus looks reachable.
    ///
    /// Returns `None` on a desktop with accessibility switched off, so
    /// `DesktopPlatform::ax()` reports the honest absence rather than a
    /// capability that fails on every call.
    #[must_use]
    pub fn probe() -> Option<Self> {
        bus_looks_reachable().then(|| Self {
            references: Arc::default(),
        })
    }
}

const REFERENCE_TTL: Duration = Duration::from_secs(AX_REFERENCE_TTL_SECS);
static NEXT_LEASE: AtomicU64 = AtomicU64::new(0);

struct ReferenceLease {
    id: String,
    deadline: Arc<Instant>,
    pid: i32,
    root_path: String,
    targets: Vec<Arc<ReferenceTarget>>,
}

#[derive(Default)]
struct ReferenceLeases {
    snapshots: VecDeque<ReferenceLease>,
    expiry: Option<AbortHandle>,
}

impl ReferenceLeases {
    fn purge(&mut self) {
        let now = Instant::now();
        while self
            .snapshots
            .front()
            .is_some_and(|lease| *lease.deadline <= now)
        {
            self.snapshots.pop_front();
        }
    }

    fn insert(
        store: &Arc<Mutex<Self>>,
        id: String,
        pid: i32,
        root_path: String,
        targets: Vec<Arc<ReferenceTarget>>,
    ) {
        let mut leases = store.lock().expect("reference leases lock");
        leases.purge();
        if leases.snapshots.len() == MAX_AX_REFERENCE_SNAPSHOTS {
            leases.snapshots.pop_front();
        }
        leases.snapshots.push_back(ReferenceLease {
            id,
            deadline: Arc::new(Instant::now() + REFERENCE_TTL),
            pid,
            root_path,
            targets,
        });
        if leases.expiry.is_none() {
            let store = Arc::downgrade(store);
            leases.expiry = Some(
                tokio::spawn(async move {
                    loop {
                        let deadline = {
                            let Some(store) = store.upgrade() else { return };
                            let mut leases = store.lock().expect("reference leases lock");
                            leases.purge();
                            let Some(oldest) = leases.snapshots.front() else {
                                leases.expiry = None;
                                return;
                            };
                            *oldest.deadline
                        };
                        tokio::time::sleep_until(deadline).await;
                    }
                })
                .abort_handle(),
            );
        }
    }
}

impl Drop for ReferenceLeases {
    fn drop(&mut self) {
        if let Some(expiry) = &self.expiry {
            expiry.abort();
        }
    }
}

fn reference_error(code: AxReferenceError, message: &str) -> DesktopError {
    DesktopError::AxReference {
        code,
        message: message.into(),
    }
}

fn stale_reference() -> DesktopError {
    reference_error(
        AxReferenceError::StaleReference,
        "The AT-SPI reference is no longer live in its issuing scope. Take a fresh snapshot.",
    )
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
    budget: Budget,
    cache: Option<&AppCache>,
) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut queue: VecDeque<(AccessibleProxy<'static>, u32)> = VecDeque::new();
    queue.push_back((root, depth));

    while let Some((proxy, remaining)) = queue.pop_front() {
        if out.len() >= walk::MAX_NODES || budget.spent() {
            break;
        }
        let path = proxy.inner().path().as_str().to_owned();
        let cached = cache.and_then(|c| c.get(&path));

        // Role and title from the bulk cache when there is one — this scan
        // covers a whole application, so it is where the per-node round trips
        // hurt most.
        let (role, title) = match cached {
            Some(item) => {
                let facts = cache::NodeFacts::from_cache(item);
                (facts.role, facts.title)
            }
            None => {
                let Ok(role) = proxy.get_role().await else {
                    continue;
                };
                (role, proxy.name().await.ok().filter(|n| !n.is_empty()))
            }
        };
        let center = center_of(&proxy, cached.map(|i| i.states), cached.map(|i| i.ifaces)).await;

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
            let child_paths = cache.map(|c| c.children_of(&path)).unwrap_or_default();
            if child_paths.is_empty() {
                if let Ok(children) = proxy.get_children().await {
                    for child_ref in children {
                        if let Ok(child) = bus.proxy_for(&child_ref).await {
                            queue.push_back((child, remaining - 1));
                        }
                    }
                }
            } else {
                for child_path in child_paths {
                    if let Ok(child) = bus.sibling_proxy(&proxy, child_path).await {
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
async fn center_of(
    proxy: &AccessibleProxy<'static>,
    cached_states: Option<atspi::StateSet>,
    cached_ifaces: Option<atspi::InterfaceSet>,
) -> Option<(f64, f64)> {
    let showing = match cached_states {
        Some(states) => states.contains(State::Showing),
        None => proxy
            .get_state()
            .await
            .is_ok_and(|s| s.contains(State::Showing)),
    };
    let ifaces = match cached_ifaces {
        Some(set) => cache::Interfaces::from_set(set, proxy),
        None => cache::Interfaces::query(proxy).await?,
    };
    let region = walk::extents_of(&ifaces, showing).await?;
    Some((
        region.x + region.width / 2.0,
        region.y + region.height / 2.0,
    ))
}

/// Resolve a locator to a single element, or say why it could not.
async fn resolve(
    bus: &Bus,
    locator: &AxLocator,
    budget: Budget,
) -> Result<(AccessibleProxy<'static>, AxElement)> {
    let app = app_or_err(bus, locator.pid).await?;
    let pid = app.pid;
    let cache = AppCache::fetch(bus, &app.root).await;
    let candidates = collect_candidates(bus, app.root, SCAN_DEPTH, budget, cache.as_ref()).await;

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
        .ok_or_else(|| {
            DesktopError::PlatformError("rank_candidates returned an out-of-range index".into())
        })?
        .proxy;

    // Re-read the matched element so the caller sees the element as it is now,
    // with its affordances — the ranking summary carries only role/title/center.
    // One element at depth 0, so the node cap never binds; the constant stands
    // in for the caller budget this path does not have.
    let mut walk = Walk::new(bus, pid, budget, walk::MAX_NODES);
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
async fn find_focused(
    bus: &Bus,
    root: AccessibleProxy<'static>,
    budget: Budget,
    cache: Option<&AppCache>,
) -> FocusScan {
    let mut queue: VecDeque<(AccessibleProxy<'static>, u32)> = VecDeque::new();
    queue.push_back((root, FOCUS_DEPTH));
    let mut saw_focusable = false;
    let mut nodes = walk::MAX_NODES;

    while let Some((proxy, remaining)) = queue.pop_front() {
        if nodes == 0 || budget.spent() {
            break;
        }
        nodes -= 1;

        let path = proxy.inner().path().as_str().to_owned();
        let cached = cache.and_then(|c| c.get(&path));
        let states = match cached {
            Some(item) => Some(item.states),
            None => proxy.get_state().await.ok(),
        };
        if let Some(states) = states {
            if states.contains(State::Focused) {
                return FocusScan::Found(proxy);
            }
            if states.contains(State::Focusable) {
                saw_focusable = true;
            }
        }

        if remaining > 0 {
            let child_paths = cache.map(|c| c.children_of(&path)).unwrap_or_default();
            if child_paths.is_empty() {
                if let Ok(children) = proxy.get_children().await {
                    for child_ref in children {
                        if let Ok(child) = bus.proxy_for(&child_ref).await {
                            queue.push_back((child, remaining - 1));
                        }
                    }
                }
            } else {
                for child_path in child_paths {
                    if let Ok(child) = bus.sibling_proxy(&proxy, child_path).await {
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
    /// which application tree is scanned — the same answer, reached directly.
    /// Without one the frontmost application is scanned, as before.
    async fn query_focused(&self, params: QueryFocusedParams) -> Result<Option<AxElement>> {
        let budget = Budget::start();
        let bus = Bus::open().await?;
        let app = app_or_err(&bus, params.pid).await?;
        let pid = app.pid;
        let cache = AppCache::fetch(&bus, &app.root).await;
        match find_focused(&bus, app.root, budget, cache.as_ref()).await {
            FocusScan::Found(proxy) => {
                // One element; `find_focused` above already bounded the search.
                let mut walk = Walk::new(&bus, pid, budget, walk::MAX_NODES);
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
        let budget = Budget::start();
        let bus = Bus::open().await?;
        let app = app_or_err(&bus, params.pid).await?;
        let pid = app.pid;
        // The budget belongs to the request: clamped into the protocol range and
        // handed to the walk, so raising it actually returns more of the tree.
        let nodes = clamp_max_nodes(params.max_nodes) as usize;
        let mut walk = Walk::new(&bus, pid, budget, nodes)
            .prefetch(&app.root)
            .await;
        let element = walk.tree(&app.root, params.max_depth).await;
        Ok(QueryResult {
            element,
            node_count: walk.spent(),
            truncated: walk.exhausted(),
        })
    }

    async fn query_by_role(&self, params: QueryByRoleParams) -> Result<QueryListResult> {
        let budget = Budget::start();
        let bus = Bus::open().await?;
        let app = app_or_err(&bus, params.pid).await?;
        let pid = app.pid;
        // Bounds the *search*, not the number of matches — see the protocol's
        // `QueryByRoleParams::max_nodes`.
        let nodes = clamp_max_nodes(params.max_nodes) as usize;
        let mut walk = Walk::new(&bus, pid, budget, nodes)
            .prefetch(&app.root)
            .await;
        let Some(tree) = walk.tree(&app.root, SCAN_DEPTH).await else {
            return Ok(QueryListResult {
                elements: Vec::new(),
                node_count: walk.spent(),
                truncated: walk.exhausted(),
            });
        };
        let mut all = Vec::new();
        flatten(&tree, &mut all);
        Ok(QueryListResult {
            elements: all.into_iter().filter(|e| e.role == params.role).collect(),
            node_count: walk.spent(),
            truncated: walk.exhausted(),
        })
    }

    async fn set_value(&self, params: SetValueParams) -> Result<AxActionResult> {
        let bus = Bus::open().await?;
        let (proxy, matched) = resolve(&bus, &params.locator, Budget::start()).await?;
        let ifaces = cache::Interfaces::query(&proxy).await.ok_or_else(|| {
            DesktopError::PlatformError(
                "The matched element stopped answering before it could be written".into(),
            )
        })?;
        write_value(&ifaces, matched, &params.value, None).await
    }

    async fn perform_action(&self, params: PerformActionParams) -> Result<AxActionResult> {
        let bus = Bus::open().await?;
        let (proxy, matched) = resolve(&bus, &params.locator, Budget::start()).await?;
        let ifaces = cache::Interfaces::query(&proxy).await.ok_or_else(|| {
            DesktopError::PlatformError(
                "The matched element stopped answering before it could be acted on".into(),
            )
        })?;
        invoke_action(&ifaces, matched, &params.action, None).await
    }

    async fn snapshot_references(&self, request: AxSnapshotRequest) -> Result<AxReferenceSnapshot> {
        if request.window_id.is_some() {
            return Err(reference_error(
                AxReferenceError::WindowScopeUnsupported,
                "Linux has no verified mapping from the public window ID to an AT-SPI root. Use a PID.",
            ));
        }
        let budget = Budget::start();
        let bus = Bus::open().await?;
        let app = match bus.app_for(request.pid).await? {
            Some(app) => app,
            None => return match request.pid {
                Some(pid) => Ok(AxReferenceSnapshot {
                    app_pid: pid,
                    window_id: None,
                    tree_available: false,
                    elements: Vec::new(),
                    focused: None,
                    tree_truncated: false,
                    truncated: false,
                    nodes_walked: 0,
                    total_interactable: 0,
                }),
                None => Err(DesktopError::NotAvailable(
                    "No frontmost application is reachable on the AT-SPI bus. Pass an explicit PID.".into(),
                )),
            },
        };
        let mut walk = Walk::new(
            &bus,
            app.pid,
            budget,
            clamp_max_nodes(request.max_nodes) as usize,
        )
        .prefetch(&app.root)
        .await;
        let (tree_available, rows) = walk
            .snapshot(&app.root, request.max_depth, request.max_elements)
            .await;
        let tree_truncated = walk.exhausted();
        let nodes_walked = walk.spent();
        let lease = NEXT_LEASE.fetch_add(1, Ordering::Relaxed).to_string();
        let mut targets = Vec::new();
        let elements: Vec<_> = rows
            .rows
            .into_iter()
            .map(|(element, target)| {
                let reference = target.map(|target| {
                    let index = targets.len() as u32;
                    targets.push(Arc::new(target));
                    NativeAxReference {
                        lease: lease.clone(),
                        index,
                    }
                });
                AxSnapshotRow { element, reference }
            })
            .collect();
        let truncated = rows.total_interactable > elements.len();
        if !targets.is_empty() {
            ReferenceLeases::insert(
                &self.references,
                lease,
                app.pid,
                app.root.inner().path().as_str().to_owned(),
                targets,
            );
        }
        Ok(AxReferenceSnapshot {
            app_pid: app.pid,
            window_id: None,
            tree_available,
            elements,
            focused: rows.focused,
            tree_truncated,
            truncated,
            nodes_walked,
            total_interactable: rows.total_interactable,
        })
    }

    async fn act_on_reference(&self, request: AxReferenceAction) -> Result<AxActionResult> {
        let (pid, root_path, lease, target) = {
            let mut leases = self.references.lock().expect("reference leases lock");
            leases.purge();
            let lease = leases
                .snapshots
                .iter()
                .find(|lease| lease.id == request.reference.lease)
                .ok_or_else(stale_reference)?;
            let target = lease
                .targets
                .get(request.reference.index as usize)
                .ok_or_else(stale_reference)?;
            (
                lease.pid,
                lease.root_path.clone(),
                Arc::downgrade(&lease.deadline),
                target.clone(),
            )
        };
        // The task stays alive through dispatch and readback if its caller is
        // cancelled. The normal await still spans the native request boundary.
        tokio::spawn(act_on_target(
            pid,
            root_path,
            lease,
            target,
            request.mutation,
        ))
        .await
        .map_err(|_| {
            reference_error(
                AxReferenceError::ActionOutcomeUnknown,
                "The AT-SPI action worker did not return a receipt.",
            )
        })?
    }
}

async fn act_on_target(
    pid: i32,
    root_path: String,
    lease: Weak<Instant>,
    target: Arc<ReferenceTarget>,
    mutation: AxMutation,
) -> Result<AxActionResult> {
    let deadline = *lease.upgrade().ok_or_else(stale_reference)?;
    let validation_deadline = deadline.min(Instant::now() + budget::AX_BUDGET);
    let (matched, set) =
        tokio::time::timeout_at(validation_deadline, verify_target(pid, &root_path, &target))
            .await
            .map_err(|_| stale_reference())??;
    let ifaces = cache::Interfaces::from_set(set, &target.proxy);
    let dispatch = ReferenceDispatch {
        owner: target.proxy.inner().destination().as_str(),
        pid,
        validation_deadline,
        lease,
    };
    match mutation {
        AxMutation::SetValue { value } => {
            if !target.writable || matched.settable != Some(true) {
                return Err(reference_error(
                    AxReferenceError::ActionUnsupported,
                    "The AT-SPI element does not offer a writable value.",
                ));
            }
            write_value(&ifaces, matched, &value, Some(&dispatch)).await
        }
        AxMutation::Perform { action } => {
            if !set.contains(atspi::Interface::Action)
                || pick_action(&target.actions, &action).is_none()
            {
                return Err(reference_error(
                    AxReferenceError::ActionUnsupported,
                    "The AT-SPI element does not offer the requested native action.",
                ));
            }
            invoke_action(&ifaces, matched, &action, Some(&dispatch)).await
        }
    }
}

async fn verify_target(
    pid: i32,
    root_path: &str,
    target: &ReferenceTarget,
) -> Result<(AxElement, atspi::InterfaceSet)> {
    let bus = Bus::open().await.map_err(|_| stale_reference())?;
    let proxy = &target.proxy;
    if !bus.owns(proxy) || !bus.provider_has_pid(proxy, pid).await {
        return Err(stale_reference());
    }
    let (role, name, states, interfaces) = tokio::try_join!(
        proxy.get_role(),
        proxy.name(),
        proxy.get_state(),
        proxy.get_interfaces(),
    )
    .map_err(|_| stale_reference())?;
    let title = Some(name).filter(|name| !name.is_empty());
    if role != target.role || title != target.title || !walk::live_state(states) {
        return Err(stale_reference());
    }
    if let Some(expected) = &target.accessible_id {
        if proxy.accessible_id().await.as_ref().ok() != Some(expected) {
            return Err(stale_reference());
        }
    }
    if !bus
        .in_scope(proxy, role, root_path, target.window.as_deref())
        .await
    {
        return Err(stale_reference());
    }
    if !walk::enabled_of(states) {
        return Err(reference_error(
            AxReferenceError::ElementDisabled,
            "The AT-SPI element is disabled.",
        ));
    }
    let secure = target.secure
        || walk::secure_of(proxy, role, title.as_deref(), None, Budget::start()).await;
    Ok((
        AxElement {
            role: roles::atspi_role_to_ax_role(role).into(),
            title,
            pid,
            secure: Some(secure),
            enabled: Some(true),
            settable: Some(walk::value_writable(states, interfaces)),
            ..AxElement::default()
        },
        interfaces,
    ))
}

async fn write_value(
    ifaces: &cache::Interfaces<'_>,
    matched: AxElement,
    wanted: &str,
    dispatch: Option<&ReferenceDispatch<'_>>,
) -> Result<AxActionResult> {
    let secure = matched.secure == Some(true);
    // Text first, then the numeric `Value` interface. A slider, dial or spin
    // button carries no text at all — it used to be advertised as
    // interactable and then refused here, which is the advertised-but-
    // disabled shape this limb exists to avoid.
    let (performed, verification) = match ifaces.editable_text().await {
        Some(editable) => {
            check_dispatch_lease(dispatch)?;
            let performed = mutation_reply(
                editable.set_text_contents(wanted).await,
                "set_text_contents",
                dispatch,
            )
            .await?;
            // Read back rather than trust the return value: a toolkit can
            // accept the call and clamp, reformat or reject the content.
            let actual = optional_readback(dispatch, async {
                ifaces.text().await?.get_text(0, -1).await.ok()
            })
            .await;
            (performed, verify_text(wanted, actual.as_deref(), secure))
        }
        None => {
            let value = ifaces.value().await.ok_or_else(|| {
                if dispatch.is_some() {
                    return reference_error(
                        AxReferenceError::ActionUnsupported,
                        "The AT-SPI element no longer offers a writable value.",
                    );
                }
                DesktopError::NotImplemented(format!(
                    "The matched {} implements neither the AT-SPI EditableText interface (for \
                         typed text) nor the Value interface (for a number), so its value cannot \
                         be written. Click it and use type_text instead.",
                    matched.role
                ))
            })?;
            let wanted: f64 = wanted.trim().parse().map_err(|_| {
                if dispatch.is_some() {
                    return DesktopError::InputFailed(
                        "The AT-SPI Value interface requires a number.".into(),
                    );
                }
                DesktopError::InputFailed(format!(
                    "The matched {} takes a number (it exposes the AT-SPI Value interface), \
                         but {:?} is not one. Pass a bare number such as \"0.5\".",
                    matched.role, wanted
                ))
            })?;
            check_dispatch_lease(dispatch)?;
            let written = if dispatch.is_some() {
                // The typed property setter discards the error reply's sender.
                // Keep the same native Set call and its receipt for reference actions.
                let proxy = value.inner();
                proxy
                    .connection()
                    .call_method(
                        Some(proxy.destination()),
                        proxy.path(),
                        Some("org.freedesktop.DBus.Properties"),
                        "Set",
                        &(
                            proxy.interface().as_str(),
                            "CurrentValue",
                            zbus::zvariant::Value::F64(wanted),
                        ),
                    )
                    .await
                    .map(|_| ())
            } else {
                value.set_current_value(wanted).await
            };
            mutation_reply(written, "set_current_value", dispatch).await?;
            let actual =
                optional_readback(dispatch, async { value.current_value().await.ok() }).await;
            (true, verify_number(wanted, actual))
        }
    };

    let mut verification = verification;
    if dispatch.is_some() && secure {
        verification.actual_preview = None;
    }
    Ok(AxActionResult {
        performed,
        path: "accessibility".into(),
        matched: Some(matched),
        verification: Some(verification),
    })
}

async fn invoke_action(
    ifaces: &cache::Interfaces<'_>,
    matched: AxElement,
    wanted: &str,
    dispatch: Option<&ReferenceDispatch<'_>>,
) -> Result<AxActionResult> {
    let action = ifaces.action().await.ok_or_else(|| {
        if dispatch.is_some() {
            return reference_error(
                AxReferenceError::ActionUnsupported,
                "The AT-SPI element no longer offers a native action.",
            );
        }
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
    let available = if let Some(dispatch) = dispatch {
        tokio::time::timeout_at(
            dispatch.validation_deadline,
            walk::canonical_action_names(&action),
        )
        .await
        .map_err(|_| stale_reference())?
    } else {
        walk::canonical_action_names(&action).await
    };

    let index = pick_action(&available, wanted).ok_or_else(|| {
        if dispatch.is_some() {
            return reference_error(
                AxReferenceError::ActionUnsupported,
                "The AT-SPI element no longer advertises the requested action.",
            );
        }
        DesktopError::NotImplemented(format!(
            "The matched {} does not support '{}'. It offers: {}. Pass one of those verbatim.",
            matched.role,
            wanted,
            if available.is_empty() {
                "nothing".to_string()
            } else {
                available.join(", ")
            }
        ))
    })?;

    check_dispatch_lease(dispatch)?;
    let performed = mutation_reply(
        action.do_action(i32::try_from(index).unwrap_or(0)).await,
        "do_action",
        dispatch,
    )
    .await?;

    Ok(AxActionResult {
        performed,
        path: "accessibility".into(),
        matched: Some(matched),
        verification: None,
    })
}

async fn optional_readback<T>(
    dispatch: Option<&ReferenceDispatch<'_>>,
    readback: impl std::future::Future<Output = Option<T>>,
) -> Option<T> {
    if dispatch.is_some() {
        tokio::time::timeout(budget::AX_BUDGET, readback)
            .await
            .ok()
            .flatten()
    } else {
        readback.await
    }
}

struct ReferenceDispatch<'a> {
    owner: &'a str,
    pid: i32,
    validation_deadline: Instant,
    lease: Weak<Instant>,
}

fn check_dispatch_lease(dispatch: Option<&ReferenceDispatch<'_>>) -> Result<()> {
    if dispatch.is_some_and(|dispatch| {
        Instant::now() >= dispatch.validation_deadline || dispatch.lease.strong_count() == 0
    }) {
        return Err(stale_reference());
    }
    Ok(())
}

async fn mutation_reply<T>(
    result: zbus::Result<T>,
    method: &str,
    dispatch: Option<&ReferenceDispatch<'_>>,
) -> Result<T> {
    let error = match result {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };
    let Some(dispatch) = dispatch else {
        return Err(DesktopError::PlatformError(format!(
            "AT-SPI {method}: {error}"
        )));
    };
    // A matching provider error is still a reply. A bus-side transport error
    // may leave a delivered invocation running. Even closing our connection
    // cannot cancel it, so keep Core's pending scope until the process exits.
    let settled = match &error {
        zbus::Error::MethodError(name, _, reply) => {
            reply
                .header()
                .sender()
                .is_some_and(|sender| sender.as_str() == dispatch.owner)
                || matches!(
                    name.as_str(),
                    "org.freedesktop.DBus.Error.NameHasNoOwner"
                        | "org.freedesktop.DBus.Error.ServiceUnknown"
                )
        }
        // Typed decoding happens after the reply and encoding before sending.
        zbus::Error::Variant(_) => true,
        _ => false,
    };
    if !settled {
        bus::wait_for_process_exit(dispatch.pid).await;
    }
    Err(reference_error(
        AxReferenceError::ActionOutcomeUnknown,
        "AT-SPI did not confirm the action outcome. Take a fresh snapshot before deciding whether to act again.",
    ))
}

/// Build the verification for a **text** write from what the element read back.
///
/// `secure` is load-bearing, not decorative. `actual_preview` used to carry up
/// to 200 characters of whatever came back — which, on a password box, is the
/// password, travelling from the limb into the model's context. macOS scrubs
/// secure values inside its Swift handler and Windows gates this very field on
/// `!secure`; Linux was the one platform that did neither, so writing to a
/// credential field and having the toolkit reformat it (a password manager
/// normalising whitespace is enough) leaked it.
///
/// On a secure element the verdict is still reported — "did it take?" is exactly
/// what the caller needs — but never the bytes.
///
/// Pure, so the redaction is provable without a desktop.
#[must_use]
fn verify_text(wanted: &str, actual: Option<&str>, secure: bool) -> AxVerification {
    match actual {
        Some(actual) if actual == wanted => AxVerification {
            state: "verified".into(),
            reason: None,
            actual_preview: None,
        },
        Some(actual) => AxVerification {
            state: "unverified".into(),
            reason: Some("the element read back a different value".into()),
            actual_preview: (!secure).then(|| actual.chars().take(200).collect()),
        },
        None => AxVerification {
            state: "unverified".into(),
            reason: Some("the element exposes no readable text to verify against".into()),
            actual_preview: None,
        },
    }
}

/// Build the verification for a **numeric** write.
///
/// A slider legitimately clamps and snaps: asking for 0.37 on a control with a
/// step of 0.1 lands on 0.4, and calling that "unverified" without saying what
/// happened would send the model into a retry loop against physics. So a value
/// the control moved to is reported as its own outcome, with the number it
/// actually holds — a number is never a secret, so there is nothing to redact.
#[must_use]
fn verify_number(wanted: f64, actual: Option<f64>) -> AxVerification {
    match actual {
        // Exact float equality would fail on values that round-tripped through
        // the bus perfectly well; the tolerance is what "the control took it"
        // means in practice.
        Some(actual) if (actual - wanted).abs() < 1e-6 => AxVerification {
            state: "verified".into(),
            reason: None,
            actual_preview: None,
        },
        Some(actual) => AxVerification {
            state: "unverified".into(),
            reason: Some(
                "the control clamped or snapped the value to its own range and step".into(),
            ),
            actual_preview: Some(actual.to_string()),
        },
        None => AxVerification {
            state: "unverified".into(),
            reason: Some("the control does not report its current value back".into()),
            actual_preview: None,
        },
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

    // ── set_value verification ──────────────────────────────────────────────

    #[test]
    fn a_secure_fields_read_back_value_never_leaves_the_limb() {
        // The bug this pins: `actual_preview` carried up to 200 characters of
        // whatever the element read back, with no secure check. Writing into a
        // credential field and having the toolkit reformat it — a password
        // manager normalising whitespace is enough — put the password into the
        // model's context. macOS scrubs secure values inside its Swift handler
        // and Windows gates this exact field on `!secure`; Linux did neither.
        let v = verify_text("hunter2", Some("hunter2 "), true);
        assert_eq!(v.state, "unverified");
        assert!(
            v.actual_preview.is_none(),
            "a secure element's bytes must not be reported: {:?}",
            v.actual_preview
        );
        // The verdict itself is still reported — "did it take?" is what the
        // caller needs, and it is not a secret.
        assert!(v.reason.is_some());
    }

    #[test]
    fn an_ordinary_field_still_reports_what_it_read_back() {
        // Withholding this everywhere would cost the model the one signal that
        // tells it a write was mangled rather than rejected.
        let v = verify_text("hello", Some("HELLO"), false);
        assert_eq!(v.state, "unverified");
        assert_eq!(v.actual_preview.as_deref(), Some("HELLO"));
    }

    #[test]
    fn a_matching_read_back_is_verified_and_carries_no_preview() {
        for secure in [true, false] {
            let v = verify_text("hello", Some("hello"), secure);
            assert_eq!(v.state, "verified");
            assert!(v.actual_preview.is_none());
            assert!(v.reason.is_none());
        }
    }

    #[test]
    fn an_unreadable_element_is_unverified_rather_than_assumed_good() {
        let v = verify_text("hello", None, false);
        assert_eq!(v.state, "unverified");
        assert!(v.actual_preview.is_none());
    }

    #[test]
    fn a_control_that_snapped_the_value_says_so_rather_than_just_failing() {
        // A slider with a step of 0.1 asked for 0.37 lands on 0.4. Reporting
        // that as a bare failure would send the model into a retry loop against
        // the widget's own arithmetic.
        let v = verify_number(0.37, Some(0.4));
        assert_eq!(v.state, "unverified");
        assert_eq!(v.actual_preview.as_deref(), Some("0.4"));
        assert!(v.reason.as_deref().unwrap().contains("clamped"));
    }

    #[test]
    fn a_numeric_write_that_landed_is_verified_within_float_tolerance() {
        // Exact equality would fail on a value that round-tripped through the
        // bus perfectly well.
        assert_eq!(verify_number(0.5, Some(0.5)).state, "verified");
        assert_eq!(verify_number(0.5, Some(0.500_000_01)).state, "verified");
        assert_eq!(verify_number(0.5, None).state, "unverified");
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

        let mut walk = Walk::new(&bus, app.pid, Budget::start(), walk::MAX_NODES)
            .prefetch(&app.root)
            .await;
        let Some(tree) = walk.tree(&app.root, 4).await else {
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
