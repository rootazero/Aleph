//! One bulk fetch instead of four round trips per node.
//!
//! # The cost this removes
//!
//! Every attribute the walk reads — role, name, states, the interface set, the
//! child list — is its own D-Bus method call into the target application. At
//! four to six calls per node that is the dominant cost of a snapshot, and it
//! is *latency*, not work: the calls are serialised round trips, so the number
//! barely moves with CPU.
//!
//! **Measured on this host** (XFCE/X11, a 70-node terminal window): walking it
//! attribute-by-attribute took **1.88 s**; `org.a11y.atspi.Cache.GetItems`
//! returned all 70 nodes — role, name, states, interfaces, parent and child
//! count — in **5.7 ms**. A 319-node application came back in 391 ms. The
//! cached values were verified field-for-field against live property reads
//! before this was wired (`tests/atspi_live.rs`).
//!
//! This is the AT-SPI counterpart of the `IUIAutomationCacheRequest` rewrite the
//! Windows limb got in 2026-07, and for the same reason: a walk that is bounded
//! by a wall-clock budget spends that budget on round trips, so cutting round
//! trips is what makes the budget generous rather than restrictive.
//!
//! # What is *not* in the cache
//!
//! Geometry, textual and numeric values, action names, descriptions and
//! accessible ids are not cache items. They stay live — but they are asked for
//! on a small minority of nodes, and [`Interfaces`] means asking costs one call
//! rather than two (the interface set comes from the cache, so there is no
//! `GetInterfaces` first).
//!
//! # When an application serves no cache
//!
//! [`AppCache::fetch`] returns `None` and the walk falls back to reading each
//! attribute live — the same shape as the Windows limb's "cached read with a
//! live-getter fallback". An uncooperative toolkit is then *slow*, never empty.

use std::collections::HashMap;

use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::action::ActionProxy;
use atspi::proxy::cache::CacheProxy;
use atspi::proxy::component::ComponentProxy;
use atspi::proxy::editable_text::EditableTextProxy;
use atspi::proxy::text::TextProxy;
use atspi::proxy::value::ValueProxy;
use atspi::{CacheItem, Interface, InterfaceSet, Role, StateSet};

use super::bus::Bus;

/// One application's accessible tree, fetched in a single call.
pub struct AppCache {
    /// Object path → its cached attributes.
    items: HashMap<String, CacheItem>,
    /// Parent object path → child object paths, in sibling order.
    children: HashMap<String, Vec<String>>,
}

impl AppCache {
    /// Fetch `app`'s whole tree, or `None` when it serves no cache.
    ///
    /// `None` is not an error condition: older Qt builds and applications with a
    /// partially loaded bridge answer nothing here, and the walk simply reads
    /// attributes live instead.
    pub async fn fetch(bus: &Bus, app: &AccessibleProxy<'static>) -> Option<Self> {
        let destination = app.inner().destination().to_owned();
        let proxy = CacheProxy::builder(bus.connection())
            .destination(destination)
            .ok()?
            .build()
            .await
            .ok()?;
        let items = proxy.get_items().await.ok()?;
        if items.is_empty() {
            return None;
        }

        // `CacheItem` carries its parent and its index among that parent's
        // children, so the tree is rebuilt without asking anyone for a child
        // list. Sorting by index keeps sibling order the same as `GetChildren`
        // would have reported — order is load-bearing, because a model reads a
        // snapshot as a list and refers back to it by position.
        let mut ordered: HashMap<String, Vec<(i32, String)>> = HashMap::new();
        let mut by_path = HashMap::with_capacity(items.len());
        for item in items {
            let path = item.object.path().as_str().to_owned();
            let parent = item.parent.path().as_str().to_owned();
            // The root's parent is itself (or the desktop); either way, letting
            // it into the child map would build a cycle the walk could not
            // terminate on.
            if parent != path {
                ordered
                    .entry(parent)
                    .or_default()
                    .push((item.index, path.clone()));
            }
            by_path.insert(path, item);
        }

        let children = ordered
            .into_iter()
            .map(|(parent, mut kids)| {
                kids.sort_by_key(|(index, _)| *index);
                (parent, kids.into_iter().map(|(_, path)| path).collect())
            })
            .collect();

        Some(Self {
            items: by_path,
            children,
        })
    }

    /// The cached attributes of the object at `path`.
    pub fn get(&self, path: &str) -> Option<&CacheItem> {
        self.items.get(path)
    }

    /// The object paths of `path`'s children, in sibling order.
    pub fn children_of(&self, path: &str) -> &[String] {
        self.children.get(path).map_or(&[], Vec::as_slice)
    }
}

/// What one node exposes, and the proxies to reach it with.
///
/// A local stand-in for `atspi`'s `ProxyExt::proxies`, which can only be built
/// by *asking* the object for its interface set. The set is already in the
/// cache, so requiring that call would put back one of the round trips this
/// module exists to remove.
pub struct Interfaces<'a> {
    set: InterfaceSet,
    proxy: zbus::Proxy<'a>,
}

impl<'a> Interfaces<'a> {
    /// Build from an interface set already known (the cached path).
    pub fn from_set(set: InterfaceSet, accessible: &AccessibleProxy<'a>) -> Self {
        Self {
            set,
            proxy: accessible.inner().clone(),
        }
    }

    /// Ask the object for its interface set (the uncached path — one call).
    pub async fn query(accessible: &AccessibleProxy<'a>) -> Option<Self> {
        let set = accessible.get_interfaces().await.ok()?;
        Some(Self::from_set(set, accessible))
    }

    // The four accessors below are written out rather than generated from one
    // generic: each proxy type carries its own builder from `#[zbus::proxy]`,
    // and they share no constructor trait to be generic over. All four follow
    // the same shape — check the cached set, then address the same object.

    /// The `Component` interface — screen geometry.
    pub async fn component(&self) -> Option<ComponentProxy<'a>> {
        if !self.set.contains(Interface::Component) {
            return None;
        }
        ComponentProxy::builder(self.proxy.connection())
            .cache_properties(zbus::proxy::CacheProperties::No)
            .destination(self.proxy.destination().to_owned())
            .ok()?
            .path(self.proxy.path().to_owned())
            .ok()?
            .build()
            .await
            .ok()
    }

    /// The `EditableText` interface — the write path for typed content.
    pub async fn editable_text(&self) -> Option<EditableTextProxy<'a>> {
        if !self.set.contains(Interface::EditableText) {
            return None;
        }
        EditableTextProxy::builder(self.proxy.connection())
            .cache_properties(zbus::proxy::CacheProperties::No)
            .destination(self.proxy.destination().to_owned())
            .ok()?
            .path(self.proxy.path().to_owned())
            .ok()?
            .build()
            .await
            .ok()
    }

    /// The `Text` interface — textual content.
    pub async fn text(&self) -> Option<TextProxy<'a>> {
        if !self.set.contains(Interface::Text) {
            return None;
        }
        TextProxy::builder(self.proxy.connection())
            .cache_properties(zbus::proxy::CacheProperties::No)
            .destination(self.proxy.destination().to_owned())
            .ok()?
            .path(self.proxy.path().to_owned())
            .ok()?
            .build()
            .await
            .ok()
    }

    /// The `Value` interface — a slider's or spin button's number.
    pub async fn value(&self) -> Option<ValueProxy<'a>> {
        if !self.set.contains(Interface::Value) {
            return None;
        }
        ValueProxy::builder(self.proxy.connection())
            .cache_properties(zbus::proxy::CacheProperties::No)
            .destination(self.proxy.destination().to_owned())
            .ok()?
            .path(self.proxy.path().to_owned())
            .ok()?
            .build()
            .await
            .ok()
    }

    /// The `Action` interface — the element's native actions.
    pub async fn action(&self) -> Option<ActionProxy<'a>> {
        if !self.set.contains(Interface::Action) {
            return None;
        }
        ActionProxy::builder(self.proxy.connection())
            .cache_properties(zbus::proxy::CacheProperties::No)
            .destination(self.proxy.destination().to_owned())
            .ok()?
            .path(self.proxy.path().to_owned())
            .ok()?
            .build()
            .await
            .ok()
    }
}

/// The role, name and states of one node, from the cache when it has them and
/// from live property reads when it does not.
pub struct NodeFacts {
    pub role: Role,
    pub states: StateSet,
    pub title: Option<String>,
    /// The accessible description, when it came free with the rest. `None` from
    /// a live query, which does not pay for it — see [`cached_title`].
    pub description: Option<String>,
    /// `None` when the interface set is unknown, which forces a live query.
    pub ifaces: Option<InterfaceSet>,
}

impl NodeFacts {
    /// From a cache item — no round trips at all.
    pub fn from_cache(item: &CacheItem) -> Self {
        Self {
            role: item.role,
            states: item.states,
            title: cached_title(item),
            description: cached_description(item),
            ifaces: Some(item.ifaces),
        }
    }

    /// From live property reads — three round trips, the pre-cache behaviour.
    ///
    /// Returns `None` only when the role cannot be read, which is the one
    /// attribute nothing downstream works without: it decides the AX
    /// vocabulary, the actionable set and the secure refusal.
    pub async fn query(proxy: &AccessibleProxy<'static>) -> Option<Self> {
        let role = proxy.get_role().await.ok()?;
        Some(Self {
            role,
            states: proxy.get_state().await.unwrap_or_default(),
            title: proxy.name().await.ok().filter(|n| !n.is_empty()),
            description: None,
            ifaces: None,
        })
    }
}

/// The element's accessible **name** — `CacheItem::short_name`, not `name`.
///
/// # The field names in `atspi` do not match what the fields hold
///
/// `Cache.GetItems` returns `… as s u s au` — interfaces, **name**, role,
/// **description**, states. The `atspi` struct declares those two strings as
/// `short_name` (before `role`) and `name` (after it), so `CacheItem::name` is
/// the *description* and `CacheItem::short_name` is the *name*.
///
/// This is not a nitpick: reading the plausible-looking field dropped the title
/// from 47 of 70 elements to 3, and a title is what a locator matches on and
/// what a model reads a snapshot by. It was caught only because the live probe
/// compares cached values against live property reads
/// (`tests/atspi_live.rs::cache_agrees_with_live`) — which is exactly why that
/// probe exists.
fn cached_title(item: &CacheItem) -> Option<String> {
    Some(item.short_name.clone()).filter(|n| !n.is_empty())
}

/// The element's accessible **description** — `CacheItem::name`. See
/// [`cached_title`] for why that is not a typo.
///
/// Free here, where a live read costs a round trip, which is what makes it
/// affordable to feed the credential-label heuristic on every text entry.
fn cached_description(item: &CacheItem) -> Option<String> {
    Some(item.name.clone()).filter(|n| !n.is_empty())
}
