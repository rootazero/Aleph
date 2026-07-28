//! Live UI Automation probes.
//!
//! Every other test in this crate is pure: role mappings, escaping rules,
//! argument assembly. None of them can answer the one question that matters
//! about the accessibility limb — *does the tree walk actually come back with
//! properties on it?* The cached walk asks the provider to attach a property set
//! to each element and reads `Cached*` getters afterwards; a request that names
//! the wrong properties, or a provider that quietly serves none, produces a tree
//! of perfectly-shaped nodes that are all `AXUnknown` with no title, no bounds
//! and no pid. That is indistinguishable from a healthy walk of an empty
//! window at the type level, and pure tests cannot see it.
//!
//! These are `#[ignore]`d because they need a real interactive desktop session
//! with a foreground window — true on a developer machine, not on a headless
//! runner. Run them deliberately:
//!
//! ```text
//! cargo test -p aleph-desktop-windows --test uia_live -- --ignored --nocapture
//! ```

#![cfg(windows)]

use aleph_desktop::traits::AccessibilityCapability;
use aleph_desktop_windows::WindowsAccessibility;
use aleph_protocol::desktop_bridge::methods::ax::{AxElement, QueryTreeParams};

/// Count nodes and how many of them carry each property, depth-first.
#[derive(Default, Debug)]
struct Census {
    nodes: usize,
    with_role: usize,
    with_title: usize,
    with_bounds: usize,
    with_pid: usize,
    secure_answered: usize,
}

fn census(node: &AxElement, out: &mut Census) {
    out.nodes += 1;
    if node.role != "AXUnknown" {
        out.with_role += 1;
    }
    if node.title.as_ref().is_some_and(|t| !t.is_empty()) {
        out.with_title += 1;
    }
    if node.bounds.is_some() {
        out.with_bounds += 1;
    }
    if node.pid != 0 {
        out.with_pid += 1;
    }
    if node.secure.is_some() {
        out.secure_answered += 1;
    }
    for child in &node.children {
        census(child, out);
    }
}

#[tokio::test]
#[ignore = "needs an interactive desktop session with a foreground window"]
async fn the_cached_walk_returns_populated_nodes() {
    let ax = WindowsAccessibility::new();
    let started = std::time::Instant::now();
    let tree = ax
        .query_tree(QueryTreeParams {
            pid: None,
            max_depth: 8,
        })
        .await
        .expect("query_tree failed")
        .expect("no tree for the foreground window");
    let elapsed = started.elapsed();

    let mut c = Census::default();
    census(&tree, &mut c);
    println!("walked {} nodes in {elapsed:?}: {c:?}", c.nodes);

    assert!(c.nodes > 1, "a real window has more than a root node");
    // The regression these guard: a cache request that names properties the
    // provider does not serve yields nodes with every field empty.
    assert!(
        c.with_pid * 2 >= c.nodes,
        "most nodes should carry a pid; got {}/{}",
        c.with_pid,
        c.nodes
    );
    assert!(
        c.with_role * 2 >= c.nodes,
        "most nodes should map to a known role; got {}/{}",
        c.with_role,
        c.nodes
    );
    assert!(c.with_title > 0, "some node should have a name");
    assert!(c.with_bounds > 0, "some node should have a rectangle");
    assert_eq!(
        c.secure_answered, c.nodes,
        "`secure` is decided for every node, never left unanswered"
    );
    // The wall-clock budget is 5s; a healthy cached walk is far under it.
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "walk took {elapsed:?}, which means the budget stopped it"
    );
}

#[tokio::test]
#[ignore = "needs an interactive desktop session with a focused control"]
async fn the_focused_element_is_enriched() {
    let ax = WindowsAccessibility::new();
    let focused = ax.query_focused().await.expect("query_focused failed");
    let Some(el) = focused else {
        println!("nothing focused; skipping");
        return;
    };
    println!("focused: {el:?}");
    // `secure` is what the type_text pre-flight gate reads; it must always be a
    // decision, and `actions`/`settable` are what `enrich_resolved` adds for the
    // one element a call resolved.
    assert!(el.secure.is_some(), "`secure` must be decided");
    assert!(
        el.settable.is_some(),
        "the resolved element must be probed for settability"
    );
}
