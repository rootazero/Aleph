//! The browser-live resources this screen has been told about.
//!
//! Source of truth is the Gateway's `browser.live.resource` /
//! `browser.live.closed` topics (`aleph_protocol::browser_live`). This catalog
//! is a *mirror*, never an authority: it records exactly what the Gateway
//! published and nothing it could infer. In particular it never derives a
//! resource from a URL, the "current tab" or a browser profile name - a
//! resource exists here only if a well-formed `PreviewResource` envelope for
//! that exact `resource_id` arrived.
//!
//! Rules (all fail-closed - a frame that breaks one leaves the catalog
//! untouched):
//!
//! * the envelope must decode as the wire type and pass
//!   [`PreviewResource::validate`];
//! * per `resource_id`, only a strictly higher `revision` replaces the stored
//!   one (a duplicate or older revision is stale);
//! * a frame for an already-stored `resource_id` whose `binding` differs is
//!   rejected whatever its revision: a resource id is bound to one target for
//!   life, so a different binding is not "a newer version of this resource";
//! * a `closed` frame removes the resource and tombstones its id, so a late or
//!   reordered resource frame cannot resurrect it. Ids are server-minted and
//!   unique (`res_...`), which is what makes the tombstone sound;
//! * a `closed` frame whose binding differs from the stored resource's is
//!   ignored (it is not about the thing we hold).
//!
//! The catalog only answers "what did the Gateway publish". Whether a viewer
//! may be opened for a resource is decided by the Gateway issuer
//! (`browser.viewer.grant.issue`), not here.

use std::collections::{HashMap, VecDeque};

use aleph_protocol::browser_live::{
    BrowserLiveClosed, PreviewResource, CLOSED_TOPIC, RESOURCE_TOPIC,
};

/// How many closed ids are remembered. Old enough ids are far past any
/// in-flight reordering window; the bound keeps a long session from growing
/// without limit.
const MAX_TOMBSTONES: usize = 256;

/// What [`BrowserResourceCatalog::apply_topic`] did with a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogUpdate {
    /// A new resource, or a strictly newer revision, was stored.
    Stored,
    /// The resource was removed (or tombstoned) by a closed frame.
    Closed,
    /// Same id, revision not newer than the stored one.
    StaleRevision,
    /// The id was closed earlier; the frame was dropped.
    AlreadyClosed,
    /// Topic is not a browser-live resource topic.
    NotBrowserTopic,
    /// Envelope missing, undecodable, invalid, or inconsistent (including a
    /// known `resource_id` re-published under a different binding). Nothing
    /// changed.
    Rejected,
}

#[derive(Debug, Default, Clone)]
pub struct BrowserResourceCatalog {
    resources: HashMap<String, PreviewResource>,
    closed: VecDeque<String>,
}

// The read accessors are the seam for the (not yet built) resource list and
// "open viewer" command; only `apply_topic` is driven by the event loop today.
#[allow(dead_code)]
impl BrowserResourceCatalog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The stored resource for `resource_id`, exactly as the Gateway sent it.
    #[must_use]
    pub fn get(&self, resource_id: &str) -> Option<&PreviewResource> {
        self.resources.get(resource_id)
    }

    /// Like [`Self::get`], but `None` once `now_ms` has reached the resource's
    /// `expires_at_ms`.
    #[must_use]
    pub fn get_unexpired(&self, resource_id: &str, now_ms: i64) -> Option<&PreviewResource> {
        self.get(resource_id)
            .filter(|resource| now_ms < resource.expires_at_ms)
    }

    /// All stored resources, ordered by `resource_id` (stable for display).
    #[must_use]
    pub fn resources(&self) -> Vec<&PreviewResource> {
        let mut all: Vec<&PreviewResource> = self.resources.values().collect();
        all.sort_by(|a, b| a.resource_id.cmp(&b.resource_id));
        all
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.resources.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }

    /// Drop the resources mirrored from a previous connection, but keep the
    /// closed tombstones.
    ///
    /// `events.subscribe` is per-connection server state and frames sent while
    /// the socket was down are gone, so a resource stored before the drop
    /// cannot be trusted and is cleared. Tombstones are kept on purpose: a
    /// `closed` already seen is still true (resource ids are unique and never
    /// reused), and a stale `resource` frame still queued from the old
    /// connection, or delivered out of order after the clear, must not
    /// resurrect it. Called on reconnect success, before re-subscribing, so the
    /// resource mirror is rebuilt only from frames the Gateway publishes on the
    /// new connection.
    pub fn clear_for_reconnect(&mut self) {
        self.resources.clear();
    }

    #[must_use]
    pub fn is_closed(&self, resource_id: &str) -> bool {
        self.closed.iter().any(|id| id == resource_id)
    }

    /// Apply one gateway topic notification. Unknown topics change nothing.
    pub fn apply_topic(&mut self, topic: &str, data: &serde_json::Value) -> CatalogUpdate {
        if topic == RESOURCE_TOPIC {
            let Some(resource) = data
                .get("resource")
                .and_then(|v| serde_json::from_value::<PreviewResource>(v.clone()).ok())
            else {
                return CatalogUpdate::Rejected;
            };
            self.apply_resource(resource)
        } else if topic == CLOSED_TOPIC {
            let Some(closed) = data
                .get("closed")
                .and_then(|v| serde_json::from_value::<BrowserLiveClosed>(v.clone()).ok())
            else {
                return CatalogUpdate::Rejected;
            };
            self.apply_closed(&closed)
        } else {
            CatalogUpdate::NotBrowserTopic
        }
    }

    fn apply_resource(&mut self, resource: PreviewResource) -> CatalogUpdate {
        if resource.resource_id.trim().is_empty()
            || resource.revision == 0
            || resource.validate().is_err()
        {
            return CatalogUpdate::Rejected;
        }
        if self.is_closed(&resource.resource_id) {
            return CatalogUpdate::AlreadyClosed;
        }
        if let Some(current) = self.resources.get(&resource.resource_id) {
            // Binding first: a different binding is a contract violation, not
            // a stale or newer revision of the stored resource.
            if current.binding != resource.binding {
                return CatalogUpdate::Rejected;
            }
            if resource.revision <= current.revision {
                return CatalogUpdate::StaleRevision;
            }
        }
        self.resources
            .insert(resource.resource_id.clone(), resource);
        CatalogUpdate::Stored
    }

    fn apply_closed(&mut self, closed: &BrowserLiveClosed) -> CatalogUpdate {
        if closed.resource_id.trim().is_empty() {
            return CatalogUpdate::Rejected;
        }
        if let Some(current) = self.resources.get(&closed.resource_id) {
            if current.binding != closed.binding {
                return CatalogUpdate::Rejected;
            }
        }
        self.resources.remove(&closed.resource_id);
        if !self.is_closed(&closed.resource_id) {
            if self.closed.len() == MAX_TOMBSTONES {
                self.closed.pop_front();
            }
            self.closed.push_back(closed.resource_id.clone());
        }
        CatalogUpdate::Closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::browser_live::{
        BrowserTargetBinding, ClosedReason, ConversationBinding, DisplayPolicy, IssuerIdentity,
        PreviewKind, PreviewProvenance, PreviewSource, RedactionStatus, ResourceBinding,
        ViewerBinding,
    };
    use serde_json::json;

    fn binding(target: &str) -> ResourceBinding {
        ResourceBinding::Browser {
            viewer: ViewerBinding {
                principal: IssuerIdentity::User {
                    user_id: "u1".into(),
                },
                task: ConversationBinding {
                    session_key: "agent:main:main".into(),
                    run_id: "run-1".into(),
                },
                target: BrowserTargetBinding {
                    profile: "default".into(),
                    browser_session_id: "bs-1".into(),
                    target_id: target.into(),
                },
            },
        }
    }

    fn resource(id: &str, revision: u64) -> PreviewResource {
        PreviewResource {
            resource_id: id.into(),
            revision,
            binding: binding("t1"),
            kind: PreviewKind::Page,
            display: DisplayPolicy::ExplicitOnly,
            source: PreviewSource::BrowserTool,
            provenance: PreviewProvenance::NotManaged,
            expires_at_ms: 2_000,
            redaction: RedactionStatus::Pending,
            companion_candidates: Vec::new(),
        }
    }

    fn resource_data(r: &PreviewResource) -> serde_json::Value {
        json!({"type": "browser_live_resource", "resource": r})
    }

    fn closed(id: &str, binding: ResourceBinding) -> serde_json::Value {
        let c = BrowserLiveClosed {
            resource_id: id.into(),
            binding,
            reason: ClosedReason::TargetClosed,
            at_ms: 1_000,
        };
        json!({"type": "browser_live_closed", "closed": c})
    }

    #[test]
    fn resource_event_is_stored_exactly() {
        let mut cat = BrowserResourceCatalog::new();
        let r = resource("res_a", 1);
        assert_eq!(
            cat.apply_topic(RESOURCE_TOPIC, &resource_data(&r)),
            CatalogUpdate::Stored
        );
        assert_eq!(cat.get("res_a"), Some(&r));
        assert_eq!(cat.len(), 1);
        assert_eq!(cat.get_unexpired("res_a", 1_999), Some(&r));
        assert_eq!(cat.get_unexpired("res_a", 2_000), None);
    }

    #[test]
    fn newer_revision_replaces_and_stale_or_duplicate_is_dropped() {
        let mut cat = BrowserResourceCatalog::new();
        cat.apply_topic(RESOURCE_TOPIC, &resource_data(&resource("res_a", 2)));

        let mut v3 = resource("res_a", 3);
        v3.redaction = RedactionStatus::Safe;
        assert_eq!(
            cat.apply_topic(RESOURCE_TOPIC, &resource_data(&v3)),
            CatalogUpdate::Stored
        );
        for stale in [3, 2, 1] {
            assert_eq!(
                cat.apply_topic(RESOURCE_TOPIC, &resource_data(&resource("res_a", stale))),
                CatalogUpdate::StaleRevision
            );
        }
        assert_eq!(cat.get("res_a"), Some(&v3));
    }

    #[test]
    fn higher_revision_with_different_binding_is_rejected() {
        let mut cat = BrowserResourceCatalog::new();
        let r = resource("res_a", 1);
        cat.apply_topic(RESOURCE_TOPIC, &resource_data(&r));

        let mut rebound = resource("res_a", 2);
        rebound.binding = binding("other-target");
        assert_eq!(
            cat.apply_topic(RESOURCE_TOPIC, &resource_data(&rebound)),
            CatalogUpdate::Rejected
        );
        assert_eq!(cat.get("res_a"), Some(&r));
    }

    #[test]
    fn clear_for_reconnect_drops_resources_but_keeps_tombstones() {
        let mut cat = BrowserResourceCatalog::new();
        cat.apply_topic(RESOURCE_TOPIC, &resource_data(&resource("res_a", 1)));
        cat.apply_topic(CLOSED_TOPIC, &closed("res_b", binding("t1")));
        assert_eq!(cat.len(), 1);
        assert!(cat.is_closed("res_b"));

        cat.clear_for_reconnect();
        assert!(cat.is_empty());
        assert!(cat.get("res_a").is_none());
        // Tombstone survives, so a stale old-connection frame cannot revive it.
        assert!(cat.is_closed("res_b"));
        assert_eq!(
            cat.apply_topic(RESOURCE_TOPIC, &resource_data(&resource("res_b", 1))),
            CatalogUpdate::AlreadyClosed
        );
        assert!(cat.is_empty());
        // New resources still rebuild from new-connection frames.
        assert_eq!(
            cat.apply_topic(RESOURCE_TOPIC, &resource_data(&resource("res_a", 1))),
            CatalogUpdate::Stored
        );
    }

    #[test]
    fn closed_removes_and_blocks_resurrection() {
        let mut cat = BrowserResourceCatalog::new();
        cat.apply_topic(RESOURCE_TOPIC, &resource_data(&resource("res_a", 1)));
        assert_eq!(
            cat.apply_topic(CLOSED_TOPIC, &closed("res_a", binding("t1"))),
            CatalogUpdate::Closed
        );
        assert!(cat.get("res_a").is_none());
        assert!(cat.is_closed("res_a"));
        // A late higher-revision frame must not bring it back.
        assert_eq!(
            cat.apply_topic(RESOURCE_TOPIC, &resource_data(&resource("res_a", 9))),
            CatalogUpdate::AlreadyClosed
        );
        assert!(cat.is_empty());
    }

    #[test]
    fn closed_before_resource_is_tombstoned() {
        let mut cat = BrowserResourceCatalog::new();
        cat.apply_topic(CLOSED_TOPIC, &closed("res_x", binding("t1")));
        assert_eq!(
            cat.apply_topic(RESOURCE_TOPIC, &resource_data(&resource("res_x", 1))),
            CatalogUpdate::AlreadyClosed
        );
    }

    #[test]
    fn closed_with_foreign_binding_is_ignored() {
        let mut cat = BrowserResourceCatalog::new();
        let r = resource("res_a", 1);
        cat.apply_topic(RESOURCE_TOPIC, &resource_data(&r));
        assert_eq!(
            cat.apply_topic(CLOSED_TOPIC, &closed("res_a", binding("other-target"))),
            CatalogUpdate::Rejected
        );
        assert_eq!(cat.get("res_a"), Some(&r));
        assert!(!cat.is_closed("res_a"));
    }

    #[test]
    fn malformed_frames_fail_closed_without_touching_state() {
        let mut cat = BrowserResourceCatalog::new();
        let r = resource("res_a", 1);
        cat.apply_topic(RESOURCE_TOPIC, &resource_data(&r));

        let mut unknown_field = serde_json::to_value(resource("res_b", 1)).unwrap();
        unknown_field["url"] = json!("https://example.com");
        let mut kind_mismatch = resource("res_c", 1);
        kind_mismatch.kind = PreviewKind::Artifact; // Browser binding + Artifact kind
        let mut empty_id = resource("res_d", 1);
        empty_id.resource_id = String::new();
        let zero_rev = resource("res_e", 0);

        let bad: Vec<(&str, serde_json::Value)> = vec![
            (RESOURCE_TOPIC, json!({})),
            (RESOURCE_TOPIC, json!({"resource": "nope"})),
            (
                RESOURCE_TOPIC,
                json!({"resource": {"resource_id": "res_z"}}),
            ),
            (RESOURCE_TOPIC, json!({"resource": unknown_field})),
            (RESOURCE_TOPIC, resource_data(&kind_mismatch)),
            (RESOURCE_TOPIC, resource_data(&empty_id)),
            (RESOURCE_TOPIC, resource_data(&zero_rev)),
            // Wrong envelope key for the topic.
            (RESOURCE_TOPIC, closed("res_a", binding("t1"))),
            (CLOSED_TOPIC, resource_data(&r)),
            (CLOSED_TOPIC, json!({"closed": {"resource_id": "res_a"}})),
            (CLOSED_TOPIC, json!(null)),
        ];
        for (topic, data) in bad {
            assert_eq!(
                cat.apply_topic(topic, &data),
                CatalogUpdate::Rejected,
                "{topic} {data}"
            );
        }
        assert_eq!(cat.len(), 1);
        assert_eq!(cat.get("res_a"), Some(&r));
        assert!(!cat.is_closed("res_a"));
    }

    #[test]
    fn other_topics_are_ignored() {
        let mut cat = BrowserResourceCatalog::new();
        assert_eq!(
            cat.apply_topic("connection.warning", &json!({})),
            CatalogUpdate::NotBrowserTopic
        );
        assert!(cat.is_empty());
    }

    #[test]
    fn tombstones_are_bounded() {
        let mut cat = BrowserResourceCatalog::new();
        for i in 0..(MAX_TOMBSTONES + 5) {
            cat.apply_topic(CLOSED_TOPIC, &closed(&format!("res_{i}"), binding("t1")));
        }
        assert_eq!(cat.closed.len(), MAX_TOMBSTONES);
        assert!(!cat.is_closed("res_0"));
        assert!(cat.is_closed(&format!("res_{}", MAX_TOMBSTONES + 4)));
    }
}
