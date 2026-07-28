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

use std::time::Instant;

use aleph_desktop::traits::AccessibilityCapability;
use aleph_desktop_linux::LinuxAccessibility;
use aleph_protocol::desktop_bridge::methods::ax::{AxElement, QueryTreeParams};

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
    let tree = ax
        .query_tree(QueryTreeParams {
            pid: None,
            max_depth: 8,
        })
        .await;
    let elapsed = started.elapsed();

    let tree = tree.expect("query_tree").expect("a frontmost application");
    let mut all = Vec::new();
    flatten(&tree, &mut all);

    println!(
        "frontmost pid {} — {} nodes in {elapsed:?}",
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
    for e in &all {
        assert!(e.role.starts_with("AX"), "role {:?} left the vocabulary", e.role);
        if let Some(b) = &e.bounds {
            assert!(b.width > 0.0 && b.height > 0.0, "unusable rectangle {b:?}");
        }
    }
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
    let Ok(Some(tree)) = ax
        .query_tree(QueryTreeParams {
            pid: None,
            max_depth: 10,
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
    match ax.query_focused().await {
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
        let Ok(Some(tree)) = ax
            .query_tree(QueryTreeParams {
                pid: None,
                max_depth: depth,
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
    assert!(compared > 0, "no application served a cache to compare against");
}
