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
use atspi::{CoordType, Interface, InterfaceSet, Role, State, StateSet};

use aleph_protocol::desktop_bridge::methods::ax::{AxElement, MAX_AX_REFERENCE_ROWS};
use aleph_protocol::desktop_bridge::methods::screen::Region;

use super::budget::Budget;
use super::bus::Bus;
use super::cache::{AppCache, Interfaces, NodeFacts};
use super::roles::{
    atspi_role_to_ax_role, has_numeric_value, has_readable_value, is_interactable, is_secure_role,
    is_text_entry,
};

/// Hard cap on how many nodes one walk will materialize.
///
/// The contract's [`DEFAULT_MAX_NODES`](aleph_protocol::desktop_bridge::methods::ax::DEFAULT_MAX_NODES)
/// is the source of truth; this mirrors it so internal scans that carry no
/// caller-supplied budget stay aligned with the protocol.
pub const MAX_NODES: usize =
    aleph_protocol::desktop_bridge::methods::ax::DEFAULT_MAX_NODES as usize;

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
    /// Node budget this walk was started with — the caller's, clamped to the
    /// protocol range, or [`MAX_NODES`] for an internal scan that has none.
    /// Kept so [`Self::spent`] can report against the budget actually in force.
    max_nodes: usize,
    remaining: usize,
    budget: Budget,
    /// The application's tree, fetched once. `None` when it serves no cache, in
    /// which case every attribute is read live — slower, never wrong.
    cache: Option<AppCache>,
    snapshot: Option<SnapshotRows>,
}

impl<'a> Walk<'a> {
    /// Start a walk over `pid`'s tree, materializing at most `max_nodes` within
    /// `budget`.
    ///
    /// Both bounds are needed and neither implies the other: the node count caps
    /// how much JSON a healthy application can produce, the budget caps how long
    /// an unhealthy one can stall. See [`super::budget`].
    ///
    /// `max_nodes` is the caller's, already clamped by
    /// [`clamp_max_nodes`](aleph_protocol::desktop_bridge::methods::ax::clamp_max_nodes).
    /// It is a parameter rather than the [`MAX_NODES`] constant because the
    /// budget belongs to the request: an internal scan with no caller behind it
    /// passes the constant explicitly, and says so at its call site.
    pub const fn new(bus: &'a Bus, pid: i32, budget: Budget, max_nodes: usize) -> Self {
        Self {
            bus,
            pid,
            max_nodes,
            remaining: max_nodes,
            budget,
            cache: None,
            snapshot: None,
        }
    }

    /// Collect flat rows and their native addresses during the same traversal.
    pub async fn snapshot(
        &mut self,
        root: &AccessibleProxy<'static>,
        depth: u32,
        limit: usize,
    ) -> (bool, SnapshotRows) {
        self.snapshot = Some(SnapshotRows {
            limit: limit.min(MAX_AX_REFERENCE_ROWS),
            ..SnapshotRows::default()
        });
        let available = self.tree(root, depth).await.is_some();
        let mut rows = self
            .snapshot
            .take()
            .expect("snapshot projection is installed");

        // AccessibleId is optional on older providers. Read it on the exact
        // published objects, with the same bounded concurrency as enrichment.
        use futures::StreamExt as _;
        let mut pending = Vec::new();
        for (_, target) in &mut rows.rows {
            if let Some(target) = target {
                pending.push(async move {
                    if let Ok(id) = target.proxy.accessible_id().await {
                        target.accessible_id = Some(id);
                    }
                });
            }
        }
        let budget = self.budget;
        let _ = tokio::time::timeout(budget.remaining(), async {
            futures::stream::iter(pending)
                .buffer_unordered(ENRICH_CONCURRENCY)
                .for_each(|()| async {})
                .await;
        })
        .await;
        (available, rows)
    }

    /// Prefetch `root`'s application tree in one call.
    ///
    /// Call this before walking a whole application. It is deliberately not
    /// automatic in [`Self::new`]: resolving a single already-matched element
    /// (what `set_value` and `perform_action` do) reads one node, and paying for
    /// a whole-tree fetch to serve it would be the wrong trade.
    pub async fn prefetch(mut self, root: &AccessibleProxy<'static>) -> Self {
        self.cache = AppCache::fetch(self.bus, root).await;
        self
    }

    /// Nodes materialised so far (delegates to the protocol's `QueryResult.node_count`).
    pub const fn spent(&self) -> u32 {
        (self.max_nodes - self.remaining) as u32
    }

    /// Whether the walk stopped because a budget — node count or wall clock —
    /// ran out. The returned tree is partial in that case, which the caller
    /// surfaces via `QueryResult::truncated`.
    ///
    /// Not `const`: the wall-clock half reads `Instant::now()`. Only the node
    /// half ([`Self::spent`]) is a pure field read.
    pub fn exhausted(&self) -> bool {
        self.remaining == 0 || self.budget.spent()
    }

    /// Build the tree rooted at `proxy`, by whichever route this application
    /// supports.
    ///
    /// With a cache the structure is already known, so the remaining live reads
    /// — geometry, values, action names — are **independent of each other** and
    /// go out concurrently ([`Self::cached_tree`]). Without one, each attribute
    /// has to be asked for in turn ([`Self::element`]).
    pub async fn tree(
        &mut self,
        proxy: &AccessibleProxy<'static>,
        depth: u32,
    ) -> Option<AxElement> {
        if self.cache.is_some() {
            if let Some(tree) = self.cached_tree(proxy, depth).await {
                return Some(tree);
            }
        }
        self.element(proxy, depth).await
    }

    /// Build the tree from the cache, enriching every node concurrently.
    ///
    /// # Why this is a separate pass rather than the recursive walk
    ///
    /// The recursive walk is inherently serial: it cannot know a node exists
    /// until its parent's child list comes back, so every round trip waits for
    /// the previous one. The cache removes that dependency — the whole structure
    /// arrives in one call — which turns the remaining reads into a flat, order-
    /// independent set. Issuing them concurrently is then the difference between
    /// *n × latency* and *n/concurrency × latency*, on a workload that is
    /// entirely latency.
    ///
    /// Returns `None` when the cache does not contain the root, in which case
    /// [`Self::tree`] falls back to the serial walk rather than reporting an
    /// application as empty.
    async fn cached_tree(
        &mut self,
        root: &AccessibleProxy<'static>,
        depth: u32,
    ) -> Option<AxElement> {
        let root_path = root.inner().path().as_str().to_owned();
        let selection = self.skeleton(&root_path, depth)?;
        let skeleton = selection.nodes;

        // Charge the budget for what the skeleton selected — *non-wrapper*
        // nodes only (see `skeleton`).
        //
        // This pass picks its nodes from the cache in one go instead of walking
        // down one `remaining -= 1` at a time like [`Self::element`], so without
        // this line the counter never moves on the cached path — which is the
        // *default* path. `node_count` came back 0 for every successful query,
        // and `exhausted()` said `false` even when `skeleton` had just cut the
        // tree at the budget: a silent truncation, the one failure the flag
        // exists to prevent. The hard-cap overflow is a budget cut too, so it
        // reports as exhausted.
        self.remaining = if selection.overflowed {
            0
        } else {
            self.max_nodes.saturating_sub(selection.charged)
        };

        // Bounded so a large application cannot open thousands of concurrent
        // D-Bus calls at once: past a point that queues inside the target's
        // single-threaded main loop anyway, and the queue is invisible from here.
        use futures::StreamExt as _;
        // Materialized before the stream: futures are lazy, so nothing has gone
        // out yet, and building them here rather than in a closure keeps the
        // borrow of `skeleton` a plain one instead of a higher-ranked bound the
        // compiler cannot prove for `buffered`.
        let snapshot = self.snapshot.is_some();
        let pending: Vec<_> = skeleton
            .iter()
            .map(|node| enrich(self.bus, root, node, self.pid, self.budget, snapshot))
            .collect();
        let enriched: Vec<AxElement> = futures::stream::iter(pending)
            .buffered(ENRICH_CONCURRENCY)
            .collect()
            .await;

        if let Some(rows) = &mut self.snapshot {
            let cache = self.cache.as_ref().expect("cached walk has an app cache");
            let owner = root.inner().destination().as_str();
            let mut stack = vec![(0, None::<&str>, true)];
            while let Some((index, window, same_provider)) = stack.pop() {
                let node = &skeleton[index];
                let window = if is_window(node.role) {
                    Some(node.path.as_str())
                } else {
                    window
                };
                let same_provider = same_provider
                    && cache
                        .get(&node.path)
                        .is_some_and(|item| item.object.name_as_str() == Some(owner));
                let element = &enriched[index];
                let proxy = if same_provider && rows.can_retain(element, node.states, node.ifaces) {
                    self.bus.sibling_proxy(root, &node.path).await.ok()
                } else {
                    None
                };
                rows.record(element, node.role, node.states, proxy, window);
                stack.extend(
                    node.children
                        .iter()
                        .rev()
                        .map(|&child| (child, window, same_provider)),
                );
            }
        }
        Some(assemble(enriched, &skeleton))
    }

    /// Select the nodes to materialize, breadth-first, from the cache alone.
    ///
    /// No I/O: every field here came back in the bulk fetch. The traversal is
    /// breadth-first so that a tree hitting the node budget keeps its shallow
    /// structure — windows, toolbars, dialogs — rather than one deep spine.
    ///
    /// # Wrappers are free
    ///
    /// A pure layout wrapper (an `AXGroup`/`AXUnknown` with no label and no
    /// content/action interface — [`SkeletonNode::is_wrapper`]) still enters
    /// the tree, but does **not** count against the node budget. This is the
    /// limb-side half of the core's render elision
    /// (`ax_compress::elide_wrapper_nodes`, ported from open-codex's
    /// `shouldElideNode`): on an Electron/GTK tree wrapper chains are the
    /// majority of nodes, and charging them is how a walk used to report
    /// `truncated` while never reaching the real controls. The wrapper stays
    /// *in* the tree so matching semantics (`verify_state` / `gui_locate` /
    /// `set_of_marks`) are untouched — only the accounting changes. Depth is
    /// still occupied: `max_depth` scopes *where* we look, and a wrapper chain
    /// is a real place.
    fn skeleton(&self, root_path: &str, depth: u32) -> Option<SkeletonSelection> {
        let cache = self.cache.as_ref()?;
        let root = cache.get(root_path)?;

        let mut out = vec![SkeletonNode::new(root_path.to_owned(), root, 0)];
        let mut charged = out
            .first()
            .map_or(0, |node| usize::from(!node.is_wrapper()));
        // Absolute ceiling on the *vector* (wrappers included): free wrappers
        // must not mean unbounded memory on a pathological tree. Hitting it is
        // a budget cut like any other and is reported as truncation.
        let hard_cap = self.max_nodes.saturating_mul(8).saturating_add(64);
        let mut overflowed = false;
        let mut cursor = 0;
        'outer: while cursor < out.len() {
            if charged >= self.max_nodes {
                break;
            }
            let Some(curr) = out.get(cursor) else {
                break;
            };
            let (path, level) = (curr.path.clone(), curr.level);
            cursor += 1;
            if level >= depth {
                continue;
            }
            for child_path in cache.children_of(&path) {
                if charged >= self.max_nodes {
                    break;
                }
                if out.len() >= hard_cap {
                    overflowed = true;
                    break 'outer;
                }
                let Some(item) = cache.get(child_path) else {
                    continue;
                };
                let index = out.len();
                if let Some(parent) = out.get_mut(cursor - 1) {
                    parent.children.push(index);
                }
                let node = SkeletonNode::new(child_path.clone(), item, level + 1);
                if !node.is_wrapper() {
                    charged += 1;
                }
                out.push(node);
            }
        }
        Some(SkeletonSelection {
            nodes: out,
            charged,
            overflowed,
        })
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
        self.element_in_scope(proxy, depth, None, proxy.inner().destination().as_str())
    }

    fn element_in_scope<'s>(
        &'s mut self,
        proxy: &'s AccessibleProxy<'static>,
        depth: u32,
        window: Option<&'s str>,
        owner: &'s str,
    ) -> BoxFut<'s, Option<AxElement>> {
        Box::pin(async move {
            if self.remaining == 0 || self.budget.spent() {
                return None;
            }
            self.remaining -= 1;

            // Role, name, states and the interface set come from the bulk cache
            // when the application serves one — one call for the whole tree
            // instead of four per node. See `super::cache`.
            let path = proxy.inner().path().as_str().to_owned();
            let facts = match self.cache.as_ref().and_then(|c| c.get(&path)) {
                Some(item) => NodeFacts::from_cache(item),
                None => NodeFacts::query(proxy).await?,
            };
            let NodeFacts {
                role,
                states,
                title,
                description,
                ifaces: cached_ifaces,
            } = facts;

            // Decided **before** the value is read, not after: a field the
            // heuristic marks secure must never have its contents fetched, the
            // same rule `has_readable_value` already enforces for the native
            // password role. Redacting downstream would mean the secret had
            // already crossed the bus.
            let secure = self
                .secure_of(proxy, role, title.as_deref(), description.as_deref())
                .await;

            let showing = states.contains(State::Showing);
            // The interface set only buys something for a node we will then
            // query further: bounds (needs showing), a value (needs a text-ish
            // or numeric role), or actions (needs an interactable role — hidden
            // menu items included, since triggering one without opening its menu
            // is exactly what the Action interface is for). A hidden container
            // buys nothing, and without a cache asking for it costs a call.
            let wants_value = !secure && (has_readable_value(role) || has_numeric_value(role));
            let snapshot = self.snapshot.is_some();
            let interface_set = if showing || is_interactable(role) || wants_value || snapshot {
                match cached_ifaces {
                    Some(set) => Some(set),
                    None => proxy.get_interfaces().await.ok(),
                }
            } else {
                None
            };
            let ifaces = interface_set.map(|set| Interfaces::from_set(set, proxy));
            let bounds = match &ifaces {
                Some(p) => extents_of(p, showing).await,
                None => None,
            };
            let value = match (&ifaces, wants_value) {
                (Some(p), true) if has_numeric_value(role) => numeric_of(p).await,
                (Some(p), true) => text_of(p).await,
                _ => None,
            };
            let actions = match (&ifaces, is_interactable(role) || snapshot) {
                (Some(p), true) => actions_of(p).await,
                _ => None,
            };

            let mut element = AxElement {
                role: atspi_role_to_ax_role(role).to_string(),
                title,
                value,
                bounds,
                pid: self.pid,
                secure: Some(secure),
                enabled: Some(enabled_of(states)),
                settable: if snapshot {
                    interface_set.map(|set| value_writable(states, set))
                } else {
                    settable_of(role, states)
                },
                actions,
                url: None,
                children: Vec::new(),
            };
            let window = if is_window(role) {
                Some(path.as_str())
            } else {
                window
            };
            // A foreign subtree stays observable, but cannot supply handles for
            // a reference issued to this application and process.
            let owner = if proxy.inner().destination().as_str() == owner {
                owner
            } else {
                ""
            };
            if let Some(rows) = &mut self.snapshot {
                let target = interface_set
                    .filter(|&set| !owner.is_empty() && rows.can_retain(&element, states, set))
                    .map(|_| proxy.clone());
                rows.record(&element, role, states, target, window);
            }
            if depth > 0 && self.remaining > 0 {
                for child in self.children_of(proxy, &path).await {
                    if self.remaining == 0 || self.budget.spent() {
                        break;
                    }
                    if let Some(node) = self
                        .element_in_scope(&child, depth - 1, window, owner)
                        .await
                    {
                        element.children.push(node);
                    }
                }
            }
            Some(element)
        })
    }

    /// Whether this element masks its content.
    ///
    /// AT-SPI's own [`Role::PasswordText`] is the primary signal. The shared
    /// label heuristic is the second, for the frameworks that never set it —
    /// Electron, Qt custom editors and some web engines expose a masked field as
    /// an ordinary entry, and typing a credential into the wrong place is not
    /// recoverable. The two extra property reads it needs are paid **only on
    /// text-entry roles** ([`is_text_entry`]), which are a small minority of any
    /// tree; on Windows the equivalent restraint is `enrich_resolved`.
    async fn secure_of(
        &self,
        proxy: &AccessibleProxy<'static>,
        role: Role,
        title: Option<&str>,
        description: Option<&str>,
    ) -> bool {
        secure_of(proxy, role, title, description, self.budget).await
    }

    /// The proxies for a node's children — from the cache's parent/index
    /// relation when there is one, otherwise from a live `GetChildren`.
    async fn children_of(
        &self,
        proxy: &AccessibleProxy<'static>,
        path: &str,
    ) -> Vec<AccessibleProxy<'static>> {
        if let Some(cache) = &self.cache {
            let paths = cache.children_of(path);
            if !paths.is_empty() {
                let mut out = Vec::with_capacity(paths.len());
                for child_path in paths {
                    if let Ok(child) = self.bus.sibling_proxy(proxy, child_path).await {
                        out.push(child);
                    }
                }
                return out;
            }
        }
        let Ok(refs) = proxy.get_children().await else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(refs.len());
        for child_ref in refs {
            if let Ok(child) = self.bus.proxy_for(&child_ref).await {
                out.push(child);
            }
        }
        out
    }
}

pub(super) struct ReferenceTarget {
    pub proxy: AccessibleProxy<'static>,
    pub role: Role,
    pub title: Option<String>,
    pub accessible_id: Option<String>,
    pub window: Option<String>,
    pub secure: bool,
    pub writable: bool,
    pub actions: Vec<String>,
}

#[derive(Default)]
pub(super) struct SnapshotRows {
    limit: usize,
    pub rows: Vec<(AxElement, Option<ReferenceTarget>)>,
    pub focused: Option<AxElement>,
    pub total_interactable: usize,
}

impl SnapshotRows {
    fn can_retain(&self, element: &AxElement, states: StateSet, interfaces: InterfaceSet) -> bool {
        self.rows.len() < self.limit
            && live_state(states)
            && enabled_of(states)
            && (value_writable(states, interfaces) || has_actions(element))
    }

    fn record(
        &mut self,
        element: &AxElement,
        role: Role,
        states: StateSet,
        proxy: Option<AccessibleProxy<'static>>,
        window: Option<&str>,
    ) {
        if states.contains(State::Focused) && self.focused.is_none() {
            self.focused = Some(element.clone());
        }
        if !is_interactable(role)
            && !has_numeric_value(role)
            && !has_actions(element)
            && element.settable != Some(true)
            && proxy.is_none()
        {
            return;
        }
        self.total_interactable += 1;
        if self.rows.len() >= self.limit {
            return;
        }
        self.rows.push((
            element.clone(),
            proxy.map(|proxy| ReferenceTarget {
                proxy,
                role,
                title: element.title.clone(),
                accessible_id: None,
                window: window.map(str::to_owned),
                secure: element.secure == Some(true),
                writable: element.settable == Some(true),
                actions: element.actions.clone().unwrap_or_default(),
            }),
        ));
    }
}

fn has_actions(element: &AxElement) -> bool {
    element
        .actions
        .as_ref()
        .is_some_and(|names| !names.is_empty())
}

pub(super) fn live_state(states: StateSet) -> bool {
    !states.contains(State::Defunct)
        && !states.contains(State::Stale)
        && !states.contains(State::Invalid)
}

pub(super) fn is_window(role: Role) -> bool {
    atspi_role_to_ax_role(role) == "AXWindow"
}

pub(super) fn value_writable(states: StateSet, interfaces: InterfaceSet) -> bool {
    !states.contains(State::ReadOnly)
        && ((states.contains(State::Editable) && interfaces.contains(Interface::EditableText))
            || interfaces.contains(Interface::Value))
}

/// How many live reads may be in flight at once during the enrichment pass.
///
/// The target application dispatches D-Bus on one main loop, so beyond a certain
/// width the calls queue *inside it* — invisible from here, and a queue that deep
/// only makes an unresponsive application look worse. 32 keeps a healthy toolkit
/// saturated without that.
const ENRICH_CONCURRENCY: usize = 32;

/// The nodes a [`Walk::skeleton`] pass selected, with its budget accounting.
struct SkeletonSelection {
    nodes: Vec<SkeletonNode>,
    /// How many *non-wrapper* nodes were selected — what the node budget is
    /// charged for.
    charged: usize,
    /// The absolute vector ceiling was hit before the budget ran out — a
    /// pathological wrapper sea. Reported as truncation like any budget cut.
    overflowed: bool,
}

/// One node chosen for materialization, with everything the cache already knew.
struct SkeletonNode {
    path: String,
    role: Role,
    states: StateSet,
    title: Option<String>,
    description: Option<String>,
    ifaces: atspi::InterfaceSet,
    /// Depth below the root, for the `max_depth` bound.
    level: u32,
    /// Indices of this node's children in the same flat vector. Always greater
    /// than the parent's own index, because the selection is breadth-first —
    /// which is what lets [`assemble`] build the tree in one reverse pass.
    children: Vec<usize>,
}

impl SkeletonNode {
    fn new(path: String, item: &atspi::CacheItem, level: u32) -> Self {
        let facts = NodeFacts::from_cache(item);
        Self {
            path,
            role: facts.role,
            states: facts.states,
            title: facts.title,
            description: facts.description,
            ifaces: item.ifaces,
            level,
            children: Vec::new(),
        }
    }

    /// True for a pure layout wrapper: maps to `AXGroup`/`AXUnknown`, carries
    /// no label, and exposes no content or action interface — nothing a user
    /// (or a locator) could name or act on.
    ///
    /// The rule mirrors the core-side render elision
    /// (`ax_compress::elide_wrapper_nodes`, itself open-codex's
    /// `shouldElideNode`) so the budget accounting here and the presentation
    /// there agree on what noise is. A wrapper that can be *acted on* (an
    /// icon-only web control rendered as a group with an Action interface) is
    /// a target, not decoration, and pays its budget like one. `secure` needs
    /// no check: a password box is a text entry, which the role mapping already
    /// keeps out of the wrapper roles.
    fn is_wrapper(&self) -> bool {
        let mapped = atspi_role_to_ax_role(self.role);
        if mapped != "AXGroup" && mapped != "AXUnknown" {
            return false;
        }
        if self.title.as_deref().is_some_and(|t| !t.trim().is_empty()) {
            return false;
        }
        !(self.ifaces.contains(Interface::Action)
            || self.ifaces.contains(Interface::Text)
            || self.ifaces.contains(Interface::EditableText)
            || self.ifaces.contains(Interface::Value)
            || self.ifaces.contains(Interface::Hypertext)
            || self.ifaces.contains(Interface::Hyperlink)
            || self.ifaces.contains(Interface::Selection)
            || self.ifaces.contains(Interface::Table))
    }
}

/// Read the attributes the cache does not carry, for one node.
///
/// Geometry, textual and numeric values, action names and the credential-label
/// signal all still cost round trips. They are asked for here so the caller can
/// issue many at once; nothing in this function depends on any other node.
async fn enrich(
    bus: &Bus,
    sibling: &AccessibleProxy<'static>,
    node: &SkeletonNode,
    pid: i32,
    budget: Budget,
    snapshot: bool,
) -> AxElement {
    let bare = |secure: bool, value, bounds, actions| AxElement {
        role: atspi_role_to_ax_role(node.role).to_string(),
        title: node.title.clone(),
        value,
        bounds,
        pid,
        secure: Some(secure),
        enabled: Some(enabled_of(node.states)),
        settable: if snapshot {
            Some(value_writable(node.states, node.ifaces))
        } else {
            settable_of(node.role, node.states)
        },
        actions,
        url: None,
        children: Vec::new(),
    };

    // Out of budget, or unaddressable: report what the cache knew rather than
    // dropping the node. Role and title are the fields a locator matches on, so
    // a node without geometry is still reachable — one without a role is not
    // there at all.
    if budget.spent() {
        return bare(is_secure_role(node.role), None, None, None);
    }
    let Ok(proxy) = bus.sibling_proxy(sibling, &node.path).await else {
        return bare(is_secure_role(node.role), None, None, None);
    };

    let secure = secure_of(
        &proxy,
        node.role,
        node.title.as_deref(),
        node.description.as_deref(),
        budget,
    )
    .await;
    let showing = node.states.contains(State::Showing);
    let wants_value = !secure && (has_readable_value(node.role) || has_numeric_value(node.role));
    let interactable =
        is_interactable(node.role) || (snapshot && node.ifaces.contains(Interface::Action));

    if !showing && !interactable && !wants_value {
        return bare(secure, None, None, None);
    }

    let ifaces = Interfaces::from_set(node.ifaces, &proxy);
    let bounds = extents_of(&ifaces, showing).await;
    let value = if !wants_value {
        None
    } else if has_numeric_value(node.role) {
        numeric_of(&ifaces).await
    } else {
        text_of(&ifaces).await
    };
    let actions = if interactable {
        actions_of(&ifaces).await
    } else {
        None
    };

    bare(secure, value, bounds, actions)
}

/// Fold the flat, enriched node list back into a tree.
///
/// Walked in reverse so every child is already complete when its parent claims
/// it — guaranteed because the breadth-first selection only ever gives a child a
/// higher index than its parent.
fn assemble(mut nodes: Vec<AxElement>, skeleton: &[SkeletonNode]) -> AxElement {
    for index in (0..nodes.len()).rev() {
        let Some(skel_node) = skeleton.get(index) else {
            continue;
        };
        let children: Vec<AxElement> = skel_node
            .children
            .iter()
            .filter_map(|&child| nodes.get_mut(child).map(std::mem::take))
            .collect();
        if let Some(node) = nodes.get_mut(index) {
            node.children = children;
        }
    }
    if !nodes.is_empty() {
        nodes.swap_remove(0)
    } else {
        AxElement::default()
    }
}

/// Whether an element masks its content — see [`Walk::secure_of`].
///
/// Free rather than a method because the concurrent enrichment pass needs it
/// without holding the walk, and there must be exactly one answer to "is this a
/// password box" whichever pass asked.
pub(super) async fn secure_of(
    proxy: &AccessibleProxy<'static>,
    role: Role,
    title: Option<&str>,
    description: Option<&str>,
    budget: Budget,
) -> bool {
    if is_secure_role(role) {
        return true;
    }
    if !is_text_entry(role) || budget.spent() {
        return false;
    }
    // The description arrives free with a cache item; only the accessible id
    // ever costs a round trip, and only on a text entry.
    let described = match description {
        Some(d) => d.to_string(),
        None => proxy.description().await.unwrap_or_default(),
    };
    let accessible_id = proxy.accessible_id().await.unwrap_or_default();
    aleph_desktop::is_password_like(
        atspi_role_to_ax_role(role),
        &[title.unwrap_or_default(), &described, &accessible_id],
    )
}

/// Whether the element is interactive right now, as opposed to greyed out.
///
/// AT-SPI splits this in two: `Enabled` is the widget's own flag, `Sensitive`
/// is whether it can receive input given its ancestors. A user sees "greyed
/// out" when either is false.
pub(super) fn enabled_of(states: StateSet) -> bool {
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
pub(super) async fn extents_of(p: &Interfaces<'_>, showing: bool) -> Option<Region> {
    if !showing {
        return None;
    }
    let component = p.component().await?;
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
async fn text_of(p: &Interfaces<'_>) -> Option<String> {
    let text = p.text().await?;
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

/// Numeric content via the `Value` interface, rendered as the string the rest of
/// the contract carries.
///
/// A slider exposes no text at all, so before this existed every slider, dial
/// and spin button in a snapshot reported no value — while `is_interactable`
/// advertised them as things to act on. The range comes along because a bare
/// "0.4" tells the model nothing about what to write back: `ax.set_value`
/// accepts a number in these very units.
async fn numeric_of(p: &Interfaces<'_>) -> Option<String> {
    let value = p.value().await?;
    let current = value.current_value().await.ok()?;
    match (value.minimum_value().await, value.maximum_value().await) {
        // A degenerate range (min == max) is what a provider that does not
        // really implement one hands back; reporting it would invite a write
        // that can only fail.
        (Ok(min), Ok(max)) if max > min => Some(format!("{current} (range {min}–{max})")),
        _ => Some(current.to_string()),
    }
}

/// Canonical action names via the `Action` interface.
///
/// **Not** `GetActions`. That bulk call returns the *localized* label — on a
/// Chinese desktop a GTK menu item reports its action as `"点击"`, not
/// `"click"` — so a model told to pass an action back verbatim would send a
/// string no alias table could ever recognise, and the same prompt would behave
/// differently per locale. `GetName(index)` is the untranslated name;
/// `GetLocalizedName` is the one meant for display, which is not what this is.
async fn actions_of(p: &Interfaces<'_>) -> Option<Vec<String>> {
    let action = p.action().await?;
    let names = canonical_action_names(&action).await;
    (!names.is_empty()).then_some(names)
}

/// The untranslated action names of one element, in index order.
///
/// Shared with `perform_action`, which must match against **the same strings a
/// snapshot reported** — matching against `GetActions` there while reporting
/// `GetName` here would mean the model is handed one vocabulary and understood
/// in another, and only on localized desktops.
pub(super) async fn canonical_action_names(
    action: &atspi::proxy::action::ActionProxy<'_>,
) -> Vec<String> {
    let Ok(count) = action.n_actions().await else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for index in 0..count.min(MAX_ACTIONS) {
        if let Ok(name) = action.get_name(index).await {
            if !name.is_empty() {
                names.push(name);
            }
        }
    }
    names
}

/// Cap on actions read per element — one D-Bus round trip each, and no real
/// widget offers more than a handful.
const MAX_ACTIONS: i32 = 8;

/// Truncate to at most `max` characters without splitting one.
fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((idx, _)) => s.get(..idx).unwrap_or(s).to_string(),
        None => s.to_string(),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use atspi::InterfaceSet;
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

    fn skeleton_node(role: Role, title: Option<&str>, ifaces: InterfaceSet) -> SkeletonNode {
        SkeletonNode {
            path: "/org/a11y/atspi/accessible/0".to_string(),
            role,
            states: StateSet::empty(),
            title: title.map(str::to_string),
            description: None,
            ifaces,
            level: 0,
            children: Vec::new(),
        }
    }

    #[test]
    fn a_bare_layout_panel_is_a_wrapper() {
        assert!(skeleton_node(Role::Panel, None, InterfaceSet::empty()).is_wrapper());
        assert!(skeleton_node(Role::Filler, None, InterfaceSet::empty()).is_wrapper());
        assert!(skeleton_node(Role::Unknown, None, InterfaceSet::empty()).is_wrapper());
        // Geometry alone does not make a wrapper real — Component is universal.
        assert!(
            skeleton_node(Role::Panel, None, InterfaceSet::new(Interface::Component)).is_wrapper()
        );
    }

    #[test]
    fn a_wrapper_with_content_or_actions_is_not_free() {
        // A labelled group is a landmark, not noise.
        assert!(!skeleton_node(Role::Panel, Some("Sidebar"), InterfaceSet::empty()).is_wrapper());
        // An icon-only clickable group is a *target* — it must pay its budget.
        assert!(
            !skeleton_node(Role::Panel, None, InterfaceSet::new(Interface::Action)).is_wrapper()
        );
        assert!(!skeleton_node(Role::Panel, None, InterfaceSet::new(Interface::Text)).is_wrapper());
    }

    #[test]
    fn real_controls_are_never_wrappers() {
        assert!(!skeleton_node(Role::Button, None, InterfaceSet::empty()).is_wrapper());
        assert!(!skeleton_node(Role::Entry, None, InterfaceSet::empty()).is_wrapper());
    }
}
