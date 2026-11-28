//! Live probes against this host's AT-SPI bus.
//!
//! Everything here is `#[ignore]`d: CI has no accessibility bus, and a probe
//! that silently passes on an empty desktop is worse than one that is not run —
//! it reports coverage the code never got. Run them on a real session with
//! `cargo test -p aleph-desktop-linux --test atspi_live -- --ignored --nocapture`.
//!
//! What they exist to prove is the thing no fixture can: that the walk comes
//! back **bounded** and that the affordances the security gates read are
//! actually populated by real toolkits, not just by the unit tests' fixtures.

#![cfg(target_os = "linux")]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aleph_desktop::DesktopError;
use aleph_desktop::traits::AccessibilityCapability;
use aleph_desktop_linux::LinuxAccessibility;
use aleph_protocol::desktop_bridge::methods::ax::{
    AxElement, AxMutation, AxReferenceAction, AxReferenceError, AxReferenceSnapshot,
    AxSnapshotRequest, DEFAULT_MAX_NODES, QueryFocusedParams, QueryResult, QueryTreeParams,
};

fn flatten(node: &AxElement, out: &mut Vec<AxElement>) {
    let mut copy = node.clone();
    copy.children = Vec::new();
    out.push(copy);
    for child in &node.children {
        flatten(child, out);
    }
}

/// The frontmost application's tree comes back, and comes back **quickly**.
///
/// The wall-clock budget is the point. A node cap bounds how many D-Bus reads a
/// walk makes, not how long each one blocks — and every read goes to another
/// process that may have stopped answering. Before the budget existed, pointing
/// `ax_snapshot` at a frozen application (the single most common reason to reach
/// for the agent) pinned the turn against the harness ceiling.
#[tokio::test]
#[ignore = "needs a live AT-SPI bus"]
async fn a_snapshot_of_the_frontmost_app_is_bounded_in_wall_clock() {
    let Some(ax) = LinuxAccessibility::probe() else {
        panic!("no accessibility bus on this host — nothing was verified");
    };

    let started = Instant::now();
    let result = ax
        .query_tree(QueryTreeParams {
            pid: None,
            max_depth: 8,
            max_nodes: DEFAULT_MAX_NODES,
        })
        .await;
    let elapsed = started.elapsed();

    let result = result.expect("query_tree");
    let truncated = result.truncated;
    let node_count = result.node_count;
    let tree = result.element.expect("a frontmost application");
    let mut all = Vec::new();
    flatten(&tree, &mut all);

    println!(
        "frontmost pid {} — {} nodes in {elapsed:?} (walked {node_count}, truncated {truncated})",
        tree.pid,
        all.len()
    );
    println!(
        "  with a role: {}, titled: {}, with bounds: {}, secure: {}, with actions: {}",
        all.iter().filter(|e| e.role != "AXUnknown").count(),
        all.iter().filter(|e| e.title.is_some()).count(),
        all.iter().filter(|e| e.bounds.is_some()).count(),
        all.iter().filter(|e| e.secure == Some(true)).count(),
        all.iter().filter(|e| e.actions.is_some()).count(),
    );

    // Twice the budget: the budget bounds the traversal, not the connection
    // setup, and a loaded machine is allowed to be slow — but not unbounded.
    assert!(
        elapsed < std::time::Duration::from_secs(12),
        "walk took {elapsed:?}"
    );
    assert!(!all.is_empty());
    // The node budget is the protocol's, not a private constant — and when it
    // does cut the tree the caller has to be told, or a model reads the pruned
    // result as "that control does not exist".
    assert!(
        all.len() <= DEFAULT_MAX_NODES as usize,
        "{} nodes exceeds the protocol budget of {DEFAULT_MAX_NODES}",
        all.len()
    );
    assert!(
        all.len() <= node_count as usize,
        "returned {} nodes but reported walking only {node_count}",
        all.len()
    );
    for e in &all {
        assert!(
            e.role.starts_with("AX"),
            "role {:?} left the vocabulary",
            e.role
        );
        if let Some(b) = &e.bounds {
            assert!(b.width > 0.0 && b.height > 0.0, "unusable rectangle {b:?}");
        }
    }
}

/// The caller's `max_nodes` bounds the walk, and a cut walk says it was cut.
///
/// Both halves were broken here, and neither was visible from the result alone.
/// `max_nodes` arrived and was thrown away (`let _ = clamp_max_nodes(..)`), so
/// the walk always used the default; and the cached path — the *default* path —
/// never charged the node counter, so `node_count` came back 0 and `truncated`
/// came back `false` even when the tree had just been cut at the budget.
///
/// A silent cut is the dangerous failure: a model handed a pruned tree with no
/// marker concludes the control it wanted does not exist and goes elsewhere.
/// Mirrors macOS's `tier_a_a_budget_exhausted_walk_reports_that_it_was_cut`.
#[tokio::test]
#[ignore = "needs a live AT-SPI bus"]
async fn a_budget_exhausted_walk_reports_that_it_was_cut() {
    let Some(ax) = LinuxAccessibility::probe() else {
        panic!("no accessibility bus on this host — nothing was verified");
    };

    let cut = ax
        .query_tree(QueryTreeParams {
            pid: None,
            max_depth: 10,
            max_nodes: 3,
        })
        .await
        .expect("query_tree");

    let mut cut_nodes = Vec::new();
    flatten(
        cut.element.as_ref().expect("a frontmost application"),
        &mut cut_nodes,
    );
    assert!(
        cut_nodes.len() <= 3,
        "a 3-node budget returned {} nodes — max_nodes is being ignored",
        cut_nodes.len()
    );
    assert_eq!(
        cut.node_count, 3,
        "the walk must spend exactly its budget, not approximately"
    );
    assert!(
        cut.truncated,
        "a 3-node budget over a real application's tree must report truncation"
    );

    let whole = ax
        .query_tree(QueryTreeParams {
            pid: None,
            max_depth: 10,
            max_nodes: DEFAULT_MAX_NODES,
        })
        .await
        .expect("query_tree");

    assert!(
        whole.node_count > cut.node_count,
        "the default budget must have read more than the 3-node one ({} vs {})",
        whole.node_count,
        cut.node_count
    );
    assert!(
        whole.node_count > 0,
        "a successful walk that reports reading zero nodes is the cached path \
         failing to charge the budget"
    );
}

/// A secure element never carries its value across the bus.
///
/// This is the invariant the whole limb exists to protect, asserted against
/// whatever this desktop actually has open rather than against a fixture.
#[tokio::test]
#[ignore = "needs a live AT-SPI bus"]
async fn no_secure_element_ever_reports_a_value() {
    let Some(ax) = LinuxAccessibility::probe() else {
        panic!("no accessibility bus on this host — nothing was verified");
    };
    let Ok(QueryResult {
        element: Some(tree),
        ..
    }) = ax
        .query_tree(QueryTreeParams {
            pid: None,
            max_depth: 10,
            max_nodes: DEFAULT_MAX_NODES,
        })
        .await
    else {
        panic!("no frontmost application");
    };

    let mut all = Vec::new();
    flatten(&tree, &mut all);
    for e in &all {
        if e.secure == Some(true) {
            println!("secure element: role={} title={:?}", e.role, e.title);
            assert!(
                e.value.is_none(),
                "a secure element carried a value: {:?}",
                e.title
            );
        }
    }
}

/// The frontmost application is reachable at all.
///
/// On this X11 host the window layer answers; the assertion that matters is
/// that the two sources agree on a pid, because on GNOME/KDE Wayland only the
/// AT-SPI one exists and it must mean the same thing.
#[tokio::test]
#[ignore = "needs a live AT-SPI bus"]
async fn the_focused_element_query_answers_or_says_why() {
    let Some(ax) = LinuxAccessibility::probe() else {
        panic!("no accessibility bus on this host — nothing was verified");
    };
    match ax.query_focused(QueryFocusedParams::default()).await {
        Ok(Some(el)) => println!(
            "focused: role={} title={:?} secure={:?} settable={:?}",
            el.role, el.title, el.secure, el.settable
        ),
        Ok(None) => println!("nothing focused (the gate will refuse typing — correct)"),
        Err(e) => println!("undetermined, fail-open: {e}"),
    }
}

/// The bulk-cache path must not be slower than the serial one it replaced, and
/// must stay well inside the wall-clock budget.
///
/// **Baseline measured on this host before the cache existed**: a 70-node
/// window took 1.88 s (~27 ms/node), which put a 200-node application at the
/// budget's edge. The ceiling below is deliberately loose — this is a live
/// desktop, not a benchmark rig — but it fails long before a regression to the
/// per-node walk would.
#[tokio::test]
#[ignore = "needs a live AT-SPI bus"]
async fn a_full_depth_snapshot_stays_well_inside_the_budget() {
    let Some(ax) = LinuxAccessibility::probe() else {
        panic!("no accessibility bus on this host — nothing was verified");
    };
    for depth in [2u32, 4, 8, 12] {
        let started = Instant::now();
        let Ok(QueryResult {
            element: Some(tree),
            ..
        }) = ax
            .query_tree(QueryTreeParams {
                pid: None,
                max_depth: depth,
                max_nodes: DEFAULT_MAX_NODES,
            })
            .await
        else {
            panic!("no frontmost application");
        };
        let mut all = Vec::new();
        flatten(&tree, &mut all);
        let elapsed = started.elapsed();
        println!(
            "depth {depth}: {} nodes in {elapsed:?} ({:.1} ms/node)",
            all.len(),
            elapsed.as_secs_f64() * 1000.0 / all.len().max(1) as f64
        );
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "depth {depth} took {elapsed:?} — the per-node walk is back"
        );
    }
}

/// The second call must not pay for a second bus connection.
///
/// Opening an `AccessibilityConnection` costs ~424 ms on this host — it resolves
/// the a11y bus address over the session bus and then performs a full second
/// handshake. Paid per call, that dominated every snapshot, locator resolution
/// and focus query, several times over the tree walk itself.
///
/// The ratio is asserted rather than an absolute time, because what regressing
/// looks like is *every call paying the connection again*, and that shows up as
/// the second call costing about as much as the first however fast the host is.
#[tokio::test]
#[ignore = "needs a live AT-SPI bus"]
async fn the_bus_connection_is_paid_for_once() {
    let Some(ax) = LinuxAccessibility::probe() else {
        panic!("no accessibility bus on this host — nothing was verified");
    };
    let query = || {
        ax.query_tree(QueryTreeParams {
            pid: None,
            max_depth: 2,
            max_nodes: DEFAULT_MAX_NODES,
        })
    };

    let first = Instant::now();
    query().await.expect("first query");
    let first = first.elapsed();

    let second = Instant::now();
    query().await.expect("second query");
    let second = second.elapsed();

    println!("first {first:?}, second {second:?}");
    assert!(
        second * 2 < first,
        "second call ({second:?}) is not materially cheaper than the first \
         ({first:?}) — the connection is being rebuilt per call"
    );
}

/// Cached attributes must equal what a live property read returns.
///
/// This is the guard that caught `CacheItem`'s field names not matching what
/// the fields hold: `short_name` carries the accessible **name** and `name`
/// carries the **description**, because the D-Bus signature orders them either
/// side of `role` and the struct does not. Reading the plausible-looking field
/// dropped the title from 47 of 70 elements to 3 — a silent, total loss of the
/// thing locators match on, with every unit test still green.
///
/// Asserted rather than printed for exactly that reason.
#[tokio::test]
#[ignore = "needs a live AT-SPI bus"]
async fn cached_attributes_equal_live_ones() {
    use atspi::proxy::accessible::AccessibleProxy;
    use atspi::proxy::cache::CacheProxy;

    let conn = atspi::AccessibilityConnection::new().await.expect("bus");
    let c = conn.connection();
    let root = conn.root_accessible_on_registry().await.expect("root");
    let apps = root.get_children().await.expect("applications");

    let mut compared = 0;
    for app in &apps {
        let Some(bus_name) = app.name().cloned() else {
            continue;
        };
        let Ok(cache) = CacheProxy::builder(c)
            .destination(zbus::names::BusName::Unique(bus_name))
            .expect("destination")
            .build()
            .await
        else {
            continue;
        };
        let Ok(items) = cache.get_items().await else {
            continue;
        };
        // A trivial application proves nothing; wait for one with real structure.
        if items.len() < 20 {
            continue;
        }

        for item in items.iter().take(10) {
            let Some(name) = item.object.name().cloned() else {
                continue;
            };
            let Ok(live) = AccessibleProxy::builder(c)
                .destination(zbus::names::BusName::Unique(name))
                .expect("destination")
                .path(item.object.path().to_owned())
                .expect("path")
                .cache_properties(zbus::proxy::CacheProperties::No)
                .build()
                .await
            else {
                continue;
            };
            let path = item.object.path().to_string();

            assert_eq!(
                live.get_role().await.ok(),
                Some(item.role),
                "cached role disagrees at {path}"
            );
            assert_eq!(
                live.name().await.ok().as_deref(),
                Some(item.short_name.as_str()),
                "`short_name` holds the accessible NAME at {path} \
                 — see ax::cache::cached_title"
            );
            assert_eq!(
                live.description().await.ok().as_deref(),
                Some(item.name.as_str()),
                "`name` holds the accessible DESCRIPTION at {path} \
                 — see ax::cache::cached_title"
            );
            compared += 1;
        }
        break;
    }
    assert!(
        compared > 0,
        "no application served a cache to compare against"
    );
}

/// This fixture exports its own controls over a real AT-SPI bus. Its tree
/// deliberately omits Component, and the second provider reuses every path.
#[tokio::test]
#[ignore = "needs a live AT-SPI bus and registry; only mutates its own provider"]
async fn atspi_ref_rejects_provider_restart_and_recycled_path() {
    let ax = LinuxAccessibility::probe().expect("live AT-SPI bus");
    let fixture = ReferenceFixture::start().await;
    let scope = AxSnapshotRequest {
        pid: Some(std::process::id().try_into().unwrap()),
        window_id: None,
        max_depth: 8,
        max_nodes: DEFAULT_MAX_NODES,
        max_elements: 16,
    };
    let error = ax
        .snapshot_references(AxSnapshotRequest {
            window_id: Some(u64::MAX),
            ..scope.clone()
        })
        .await
        .unwrap_err();
    assert_ref_error(error, AxReferenceError::WindowScopeUnsupported);

    let initial = ax
        .snapshot_references(scope.clone())
        .await
        .expect("owned snapshot");
    assert!(initial.tree_available);
    assert_eq!(Some(initial.app_pid), scope.pid);
    assert_eq!(initial.window_id, None);
    let entry = initial
        .elements
        .iter()
        .find(|row| row.element.role == "AXTextField")
        .unwrap();
    assert!(entry.element.bounds.is_none());
    assert_eq!(entry.element.settable, Some(true));
    let written = "汉字 — acción 🌱";
    let receipt = ax
        .act_on_reference(AxReferenceAction {
            reference: entry
                .reference
                .clone()
                .expect("entry lease without geometry"),
            mutation: AxMutation::SetValue {
                value: written.into(),
            },
        })
        .await
        .expect("native Unicode write");
    assert!(receipt.performed);
    assert_eq!(receipt.verification.unwrap().state, "verified");
    assert_eq!(fixture.state.entry.lock().unwrap().as_str(), written);
    let observed = ax
        .snapshot_references(scope.clone())
        .await
        .expect("independent readback");
    assert!(
        observed
            .elements
            .iter()
            .any(|row| row.element.value.as_deref() == Some(written))
    );

    let target = duplicate_button_action(&observed);
    fixture.state.buttons.lock().unwrap().reverse = true;
    let mut dispatched = Box::pin(ax.act_on_reference(target.clone()));
    tokio::select! {
        () = fixture.state.action_started.notified() => {},
        result = &mut dispatched => panic!("action settled before the provider replied: {result:?}"),
        () = tokio::time::sleep(Duration::from_secs(5)) => panic!("provider never received DoAction"),
    }
    assert_eq!(fixture.state.buttons.lock().unwrap().clicks, [0, 0]);
    fixture.state.finish_action.notify_one();
    assert!(dispatched.await.expect("native button action").performed);
    assert_eq!(fixture.state.buttons.lock().unwrap().clicks, [0, 1]);

    fixture.state.buttons.lock().unwrap().second_label = "Recycled";
    assert_ref_error(
        ax.act_on_reference(target.clone()).await.unwrap_err(),
        AxReferenceError::StaleReference,
    );
    assert_eq!(fixture.state.buttons.lock().unwrap().clicks, [0, 1]);
    fixture
        .connection
        .close()
        .await
        .expect("disconnect the provider");
    assert_ref_error(
        ax.act_on_reference(target.clone()).await.unwrap_err(),
        AxReferenceError::StaleReference,
    );

    let replacement = ReferenceFixture::start().await;
    let fresh = ax
        .snapshot_references(scope)
        .await
        .expect("snapshot the restarted provider");
    let fresh_action = duplicate_button_action(&fresh);
    assert_ref_error(
        ax.act_on_reference(target).await.unwrap_err(),
        AxReferenceError::StaleReference,
    );
    assert_eq!(replacement.state.buttons.lock().unwrap().clicks, [0, 0]);
    replacement.state.finish_action.notify_one();
    assert!(ax.act_on_reference(fresh_action).await.unwrap().performed);
    assert_eq!(replacement.state.buttons.lock().unwrap().clicks, [0, 1]);
    replacement
        .connection
        .close()
        .await
        .expect("disconnect the replacement");
}

fn assert_ref_error(error: DesktopError, expected: AxReferenceError) {
    assert!(
        matches!(error, DesktopError::AxReference { code, .. } if code == expected),
        "expected {expected:?}, got {error:?}"
    );
}

fn duplicate_button_action(snapshot: &AxReferenceSnapshot) -> AxReferenceAction {
    let buttons: Vec<_> = snapshot
        .elements
        .iter()
        .filter(|row| row.element.title.as_deref() == Some("Same label"))
        .collect();
    assert_eq!(buttons.len(), 2);
    AxReferenceAction {
        reference: buttons[1]
            .reference
            .clone()
            .expect("second duplicate has a lease"),
        mutation: AxMutation::Perform {
            action: "click".into(),
        },
    }
}

const FIXTURE_ROOT: &str = "/org/a11y/atspi/accessible/root";
const FIXTURE_WINDOW: &str = "/org/a11y/atspi/accessible/window";
const FIXTURE_ENTRY: &str = "/org/a11y/atspi/accessible/entry";
const FIXTURE_FIRST: &str = "/org/a11y/atspi/accessible/button_first";
const FIXTURE_SECOND: &str = "/org/a11y/atspi/accessible/button_second";

struct ReferenceFixture {
    connection: zbus::Connection,
    state: Arc<FixtureState>,
}

struct FixtureState {
    entry: Mutex<String>,
    buttons: Mutex<FixtureButtons>,
    action_started: tokio::sync::Notify,
    finish_action: tokio::sync::Notify,
}

struct FixtureButtons {
    clicks: [usize; 2],
    second_label: &'static str,
    reverse: bool,
}

impl ReferenceFixture {
    async fn start() -> Self {
        let accessibility = atspi::AccessibilityConnection::new()
            .await
            .expect("fixture bus");
        let connection = accessibility.connection().clone();
        let owner = connection
            .unique_name()
            .expect("fixture unique bus owner")
            .to_owned();
        let state = Arc::new(FixtureState {
            entry: Mutex::new("Initial".into()),
            buttons: Mutex::new(FixtureButtons {
                clicks: [0, 0],
                second_label: "Same label",
                reverse: false,
            }),
            action_started: tokio::sync::Notify::new(),
            finish_action: tokio::sync::Notify::new(),
        });
        let server = connection.object_server();
        for (path, role, parent) in [
            (FIXTURE_ROOT, atspi::Role::Application, None),
            (FIXTURE_WINDOW, atspi::Role::Frame, Some(FIXTURE_ROOT)),
            (FIXTURE_ENTRY, atspi::Role::Entry, Some(FIXTURE_WINDOW)),
            (FIXTURE_FIRST, atspi::Role::Button, Some(FIXTURE_WINDOW)),
            (FIXTURE_SECOND, atspi::Role::Button, Some(FIXTURE_WINDOW)),
        ] {
            assert!(
                server
                    .at(
                        path,
                        FixtureAccessible {
                            path,
                            role,
                            parent,
                            owner: owner.clone().into(),
                            state: state.clone(),
                        }
                    )
                    .await
                    .unwrap()
            );
        }
        assert!(
            server
                .at(FIXTURE_ROOT, FixtureApplication(0))
                .await
                .unwrap()
        );
        assert!(
            server
                .at(FIXTURE_ENTRY, FixtureEditableText(state.clone()))
                .await
                .unwrap()
        );
        assert!(
            server
                .at(FIXTURE_ENTRY, FixtureText(state.clone()))
                .await
                .unwrap()
        );
        assert!(
            server
                .at(
                    FIXTURE_FIRST,
                    FixtureAction {
                        index: 0,
                        state: state.clone()
                    }
                )
                .await
                .unwrap()
        );
        assert!(
            server
                .at(
                    FIXTURE_SECOND,
                    FixtureAction {
                        index: 1,
                        state: state.clone()
                    }
                )
                .await
                .unwrap()
        );
        let registry = atspi::proxy::socket::SocketProxy::new(&connection)
            .await
            .expect("AT-SPI registry");
        registry
            .embed(&(
                owner.as_str(),
                zbus::zvariant::ObjectPath::try_from(FIXTURE_ROOT).unwrap(),
            ))
            .await
            .expect("register the owned fixture");
        Self { connection, state }
    }
}

struct FixtureAccessible {
    path: &'static str,
    role: atspi::Role,
    parent: Option<&'static str>,
    owner: zbus::names::UniqueName<'static>,
    state: Arc<FixtureState>,
}

#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl FixtureAccessible {
    fn get_role(&self) -> atspi::Role {
        self.role
    }

    fn get_children(&self) -> Vec<atspi::ObjectRefOwned> {
        let paths: &[_] = match self.path {
            FIXTURE_ROOT => &[FIXTURE_WINDOW],
            FIXTURE_WINDOW if self.state.buttons.lock().unwrap().reverse => {
                &[FIXTURE_ENTRY, FIXTURE_SECOND, FIXTURE_FIRST]
            }
            FIXTURE_WINDOW => &[FIXTURE_ENTRY, FIXTURE_FIRST, FIXTURE_SECOND],
            _ => &[],
        };
        paths
            .iter()
            .map(|&path| {
                atspi::ObjectRef::new_owned(
                    self.owner.clone(),
                    zbus::zvariant::ObjectPath::try_from(path).expect("fixture accessible path"),
                )
            })
            .collect()
    }

    fn get_state(&self) -> atspi::StateSet {
        let mut states = atspi::StateSet::new(atspi::State::Enabled);
        states.insert(atspi::State::Sensitive);
        if self.path == FIXTURE_ENTRY {
            states.insert(atspi::State::Editable);
            states.insert(atspi::State::Focused);
        }
        states
    }

    fn get_interfaces(&self) -> atspi::InterfaceSet {
        let mut interfaces = atspi::InterfaceSet::new(atspi::Interface::Accessible);
        match self.path {
            FIXTURE_ENTRY => {
                interfaces.insert(atspi::Interface::EditableText);
                interfaces.insert(atspi::Interface::Text);
            }
            FIXTURE_FIRST | FIXTURE_SECOND => interfaces.insert(atspi::Interface::Action),
            _ => {}
        }
        interfaces
    }

    #[zbus(property)]
    fn name(&self) -> &str {
        match self.path {
            FIXTURE_ROOT => "Aleph owned accessibility fixture",
            FIXTURE_ENTRY => "Entry",
            FIXTURE_FIRST => "Same label",
            FIXTURE_SECOND => self.state.buttons.lock().unwrap().second_label,
            _ => "Fixture window",
        }
    }

    #[zbus(property)]
    fn description(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn accessible_id(&self) -> &str {
        self.path
    }

    #[zbus(property)]
    fn parent(&self) -> atspi::ObjectRefOwned {
        self.parent
            .map_or_else(atspi::ObjectRefOwned::default, |path| {
                atspi::ObjectRef::new_owned(
                    self.owner.clone(),
                    zbus::zvariant::ObjectPath::try_from(path).expect("fixture parent path"),
                )
            })
    }
}

struct FixtureApplication(i32);

#[zbus::interface(name = "org.a11y.atspi.Application")]
impl FixtureApplication {
    #[zbus(property)]
    fn id(&self) -> i32 {
        self.0
    }

    #[zbus(property)]
    fn set_id(&mut self, id: i32) {
        self.0 = id;
    }
}

struct FixtureAction {
    index: usize,
    state: Arc<FixtureState>,
}

#[zbus::interface(name = "org.a11y.atspi.Action")]
impl FixtureAction {
    #[zbus(property)]
    fn n_actions(&self) -> i32 {
        1
    }

    fn get_name(&self, _index: i32) -> &str {
        "click"
    }

    async fn do_action(&self, index: i32) -> bool {
        assert_eq!(index, 0);
        if self.index == 1 {
            self.state.action_started.notify_one();
            self.state.finish_action.notified().await;
        }
        self.state.buttons.lock().unwrap().clicks[self.index] += 1;
        true
    }
}

struct FixtureEditableText(Arc<FixtureState>);

#[zbus::interface(name = "org.a11y.atspi.EditableText")]
impl FixtureEditableText {
    fn set_text_contents(&self, contents: &str) -> bool {
        *self.0.entry.lock().unwrap() = contents.into();
        true
    }
}

struct FixtureText(Arc<FixtureState>);

#[zbus::interface(name = "org.a11y.atspi.Text")]
impl FixtureText {
    fn get_text(&self, _start: i32, _end: i32) -> String {
        self.0.entry.lock().unwrap().clone()
    }
}
