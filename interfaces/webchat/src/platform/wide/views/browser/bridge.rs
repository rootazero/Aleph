//! Gateway bus → [`BrowserWorkspace`] bridge.
//!
//! Decodes the two `browser.live.*` topics and applies them to the model. It
//! is deliberately thin: every acceptance rule (principal/task/target binding,
//! revision monotonicity, close tombstones, fail-closed redaction) lives in
//! [`BrowserViewerModel`]; this file only decides *which bytes are a candidate*.
//!
//! # What is accepted
//!
//! * The topic must equal [`RESOURCE_TOPIC`] / [`CLOSED_TOPIC`] exactly — no
//!   prefix, no wildcard, no case folding.
//! * The payload must be the NESTED form the gateway emits:
//!   `data.resource` / `data.closed`. A top-level resource/closed object, a
//!   missing field, or a value that does not decode as the protocol type is
//!   ignored. The envelope's own `type` tag is not consulted: the nested,
//!   `deny_unknown_fields` protocol type is the contract.
//! * The nested object is decoded as-is. Nothing is inferred from a URL, a
//!   profile or a "current tab".
//!
//! Anything else is ignored silently. Payloads are never logged: even a
//! rejected event may carry binding identifiers.
//!
//! # Seeding after (re)connect
//!
//! Bus frames are never replayed: a resource published before this socket
//! existed (e.g. before a Panel reload) would otherwise stay invisible until
//! its next revision. [`seed_from_gateway`] asks the Gateway registry for the
//! verified caller's exact live resources (`browser.viewer.resource.list`)
//! and applies them through the SAME model acceptance path as a bus frame
//! ([`BrowserViewerModel::bind_resource`]). Fences:
//!
//! * the caller (user + connection epoch + gateway) must still be current
//!   when the answer lands, else it is dropped;
//! * any browser-live frame decoded while the request was in flight makes the
//!   snapshot possibly older than the model, so it is discarded and re-asked
//!   (bounded), never merged;
//! * a bound model only accepts a newer revision of its own `resource_id`;
//!   an unbound model binds only when the list names exactly one resource
//!   (several are ambiguous: nothing is chosen by URL, tab or order);
//! * absence from the list never unbinds (close notices do that).
//!
//! Not here: grant issue/consume/exchange or the viewer socket (see
//! [`super`]). A bound resource alone never makes the viewer live.

use aleph_protocol::browser_live::{
    BrowserLiveClosed, PreviewResource, CLOSED_TOPIC, RESOURCE_TOPIC,
};
use leptos::prelude::*;
use serde_json::Value;

use super::model::{BrowserViewerModel, BrowserWorkspace, ModelError};
use crate::api::browser_viewer::{BrowserViewerApi, ViewerCaller};
use crate::context::{DashboardState, GatewayEvent};

/// List requests per seed before giving up to the live bus. Each retry is
/// caused by a browser-live frame landing mid-request, which itself updates
/// the model, so giving up loses nothing the bus does not already carry.
pub const SEED_ATTEMPTS: usize = 3;

/// A decoded, not yet validated, browser-live bus event.
#[derive(Clone, Debug, PartialEq)]
pub enum BrowserBusEvent {
    Resource(PreviewResource),
    Closed(BrowserLiveClosed),
}

/// What [`apply_browser_event`] did with an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserEventOutcome {
    /// Not a well-formed browser-live event; nothing was touched.
    Ignored,
    /// Decoded and accepted by the model.
    Applied,
    /// Decoded but refused by the model's own rules; the model is unchanged.
    Rejected(ModelError),
}

/// Decode a bus event. `None` for any event that is not exactly a nested
/// resource/closed envelope on its own topic.
#[must_use]
pub fn decode_browser_event(topic: &str, data: &Value) -> Option<BrowserBusEvent> {
    match topic {
        RESOURCE_TOPIC => {
            let nested = data.as_object()?.get("resource")?;
            serde_json::from_value(nested.clone())
                .ok()
                .map(BrowserBusEvent::Resource)
        }
        CLOSED_TOPIC => {
            let nested = data.as_object()?.get("closed")?;
            serde_json::from_value(nested.clone())
                .ok()
                .map(BrowserBusEvent::Closed)
        }
        _ => None,
    }
}

/// Apply a decoded event to a model.
pub fn apply_to_model(
    model: &mut BrowserViewerModel,
    event: BrowserBusEvent,
) -> Result<(), ModelError> {
    match event {
        BrowserBusEvent::Resource(resource) => model.bind_resource(resource),
        BrowserBusEvent::Closed(closed) => model.apply_closed(&closed),
    }
}

/// Decode `event` and, if it is a browser-live envelope, apply it to the
/// workspace model. Synchronous and panic-free (a disposed signal is a no-op).
pub fn apply_browser_event(
    event: &GatewayEvent,
    workspace: &BrowserWorkspace,
) -> BrowserEventOutcome {
    let Some(decoded) = decode_browser_event(&event.topic, &event.data) else {
        return BrowserEventOutcome::Ignored;
    };
    workspace.note_frame();
    workspace
        .model
        .try_update(|model| apply_to_model(model, decoded))
        .map_or(BrowserEventOutcome::Ignored, |result| match result {
            Ok(()) => BrowserEventOutcome::Applied,
            Err(err) => BrowserEventOutcome::Rejected(err),
        })
}

/// What a Gateway list snapshot did to the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeedOutcome {
    /// The model changed (bound, or refreshed to a newer revision).
    Applied,
    /// Nothing to do: empty list, already current, or bound to a resource the
    /// list does not name.
    Unchanged,
    /// Unbound and the list names this many resources: none is chosen.
    Ambiguous(usize),
    /// A browser-live frame landed while the list was in flight; the snapshot
    /// was discarded untouched.
    Stale,
    /// The model's own rules refused the listed resource (e.g. a tombstone).
    Rejected(ModelError),
}

/// Apply a Gateway list snapshot to a model (pure; see the module doc).
pub fn seed_model(model: &mut BrowserViewerModel, listed: Vec<PreviewResource>) -> SeedOutcome {
    let candidate = match model.bound() {
        Some(bound) => {
            let id = &bound.resource().resource_id;
            match listed.into_iter().find(|r| &r.resource_id == id) {
                Some(resource) => resource,
                None => return SeedOutcome::Unchanged,
            }
        }
        None => {
            let count = listed.len();
            let mut listed = listed.into_iter();
            match (listed.next(), count) {
                (None, _) => return SeedOutcome::Unchanged,
                (Some(only), 1) => only,
                (Some(_), n) => return SeedOutcome::Ambiguous(n),
            }
        }
    };
    let before = model.clone();
    match model.bind_resource(candidate) {
        Ok(()) if *model == before => SeedOutcome::Unchanged,
        Ok(()) => SeedOutcome::Applied,
        Err(err) => SeedOutcome::Rejected(err),
    }
}

/// Apply a list snapshot to the workspace iff no browser-live frame was
/// decoded since `frames_at_request` (the [`BrowserWorkspace::frame_seq`]
/// read before the request was sent). Writes the signal only on a change.
pub fn seed_workspace(
    workspace: &BrowserWorkspace,
    frames_at_request: u64,
    listed: Vec<PreviewResource>,
) -> SeedOutcome {
    if workspace.frame_seq() != frames_at_request {
        return SeedOutcome::Stale;
    }
    let Some(mut next) = workspace.model.try_get_untracked() else {
        return SeedOutcome::Unchanged;
    };
    let outcome = seed_model(&mut next, listed);
    if outcome == SeedOutcome::Applied {
        workspace.model.try_set(next);
    }
    outcome
}

/// Seed the workspace from the Gateway's exact list for `caller`, fenced on
/// the caller still being current and on no frame landing mid-request.
/// `Err` carries only a fixed, sanitized reason (never a payload).
pub async fn seed_from_gateway(
    state: DashboardState,
    workspace: BrowserWorkspace,
    caller: ViewerCaller,
) -> Result<SeedOutcome, String> {
    let mut outcome = SeedOutcome::Stale;
    for _ in 0..SEED_ATTEMPTS {
        let frames = workspace.frame_seq();
        let listed = BrowserViewerApi::list_resources(&state, &caller).await?;
        if !ViewerCaller::still_valid(Some(&caller), ViewerCaller::current(&state).as_ref()) {
            return Err("caller changed".to_string());
        }
        outcome = seed_workspace(&workspace, frames, listed);
        if outcome != SeedOutcome::Stale {
            break;
        }
    }
    Ok(outcome)
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

    fn viewer(target_id: &str) -> ViewerBinding {
        ViewerBinding {
            principal: IssuerIdentity::User {
                user_id: "user-42".to_string(),
            },
            task: ConversationBinding {
                session_key: "agent:operator:browser".to_string(),
                run_id: "run-7".to_string(),
            },
            target: BrowserTargetBinding {
                profile: "default".to_string(),
                browser_session_id: "browser-session-3".to_string(),
                target_id: target_id.to_string(),
            },
        }
    }

    fn resource(revision: u64) -> PreviewResource {
        PreviewResource {
            resource_id: "resource-5".to_string(),
            revision,
            binding: ResourceBinding::Browser {
                viewer: viewer("target-9"),
            },
            kind: PreviewKind::Page,
            display: DisplayPolicy::AutoRevealCurrentRun,
            source: PreviewSource::BrowserTool,
            provenance: PreviewProvenance::NotManaged,
            expires_at_ms: 2_000,
            redaction: RedactionStatus::Safe,
            companion_candidates: Vec::new(),
        }
    }

    fn bound(workspace: &BrowserWorkspace) -> bool {
        workspace.model.read_untracked().bound().is_some()
    }

    fn closed(target_id: &str) -> BrowserLiveClosed {
        BrowserLiveClosed {
            resource_id: "resource-5".to_string(),
            binding: ResourceBinding::Browser {
                viewer: viewer(target_id),
            },
            reason: ClosedReason::Expired,
            at_ms: 5,
        }
    }

    fn resource_data(revision: u64) -> Value {
        json!({
            "type": "browser_live_resource",
            "resource": serde_json::to_value(resource(revision)).unwrap(),
        })
    }

    fn closed_data(target_id: &str) -> Value {
        json!({
            "type": "browser_live_closed",
            "closed": serde_json::to_value(closed(target_id)).unwrap(),
        })
    }

    #[test]
    fn nested_resource_envelope_decodes_and_binds() {
        let decoded = decode_browser_event(RESOURCE_TOPIC, &resource_data(2));
        assert_eq!(decoded, Some(BrowserBusEvent::Resource(resource(2))));

        let mut model = BrowserViewerModel::default();
        apply_to_model(&mut model, decoded.unwrap()).unwrap();
        let bound = model.bound().expect("resource must bind");
        assert_eq!(bound.resource().resource_id, "resource-5");
    }

    #[test]
    fn nested_closed_envelope_decodes_and_unbinds_matching_resource() {
        let decoded = decode_browser_event(CLOSED_TOPIC, &closed_data("target-9"));
        assert_eq!(decoded, Some(BrowserBusEvent::Closed(closed("target-9"))));

        let mut model = BrowserViewerModel::default();
        model.bind_resource(resource(2)).unwrap();
        apply_to_model(&mut model, decoded.unwrap()).unwrap();
        assert!(model.bound().is_none());
    }

    #[test]
    fn closed_for_a_different_binding_is_rejected_and_changes_nothing() {
        let mut model = BrowserViewerModel::default();
        model.bind_resource(resource(2)).unwrap();
        let before = model.clone();
        let decoded = decode_browser_event(CLOSED_TOPIC, &closed_data("other-target")).unwrap();
        assert!(apply_to_model(&mut model, decoded).is_err());
        assert_eq!(model, before);
    }

    #[test]
    fn missing_nested_field_is_ignored() {
        for data in [
            json!({}),
            json!({ "type": "browser_live_resource" }),
            json!({ "closed": serde_json::to_value(closed("target-9")).unwrap() }),
            Value::Null,
            json!([]),
        ] {
            assert_eq!(decode_browser_event(RESOURCE_TOPIC, &data), None, "{data}");
        }
        for data in [
            json!({}),
            json!({ "resource": serde_json::to_value(resource(2)).unwrap() }),
            Value::Null,
        ] {
            assert_eq!(decode_browser_event(CLOSED_TOPIC, &data), None, "{data}");
        }
    }

    #[test]
    fn malformed_nested_payload_is_ignored() {
        for nested in [
            json!(null),
            json!("resource-5"),
            json!(42),
            json!([]),
            json!({}),
            json!({ "resource_id": "resource-5" }),
        ] {
            assert_eq!(
                decode_browser_event(RESOURCE_TOPIC, &json!({ "resource": nested.clone() })),
                None
            );
            assert_eq!(
                decode_browser_event(CLOSED_TOPIC, &json!({ "closed": nested })),
                None
            );
        }

        // Unknown field inside a `deny_unknown_fields` protocol type.
        let mut value = serde_json::to_value(closed("target-9")).unwrap();
        value["extra"] = json!("smuggled");
        assert_eq!(
            decode_browser_event(CLOSED_TOPIC, &json!({ "closed": value })),
            None
        );
    }

    #[test]
    fn wrong_topic_is_ignored_even_with_a_valid_envelope() {
        for topic in [
            "",
            "browser.live",
            "browser.live.*",
            "browser.live.resource.extra",
            "Browser.Live.Resource",
            "browser.live.snapshot",
            "artifact.changed",
        ] {
            assert_eq!(
                decode_browser_event(topic, &resource_data(2)),
                None,
                "{topic}"
            );
            assert_eq!(
                decode_browser_event(topic, &closed_data("target-9")),
                None,
                "{topic}"
            );
        }
        // Right payload, swapped topic.
        assert_eq!(decode_browser_event(CLOSED_TOPIC, &resource_data(2)), None);
        assert_eq!(
            decode_browser_event(RESOURCE_TOPIC, &closed_data("target-9")),
            None
        );
    }

    #[test]
    fn top_level_payload_is_not_accepted() {
        let top_resource = serde_json::to_value(resource(2)).unwrap();
        let top_closed = serde_json::to_value(closed("target-9")).unwrap();
        assert_eq!(decode_browser_event(RESOURCE_TOPIC, &top_resource), None);
        assert_eq!(decode_browser_event(CLOSED_TOPIC, &top_closed), None);
    }

    fn other(revision: u64) -> PreviewResource {
        let mut r = resource(revision);
        r.resource_id = "resource-6".to_string();
        r.binding = ResourceBinding::Browser {
            viewer: viewer("target-10"),
        };
        r
    }

    fn bound_id(model: &BrowserViewerModel) -> Option<(String, u64)> {
        model
            .bound()
            .map(|b| (b.resource().resource_id.clone(), b.resource().revision))
    }

    #[test]
    fn seed_binds_an_unbound_model_only_from_exactly_one_listed_resource() {
        let mut model = BrowserViewerModel::default();
        assert_eq!(seed_model(&mut model, vec![]), SeedOutcome::Unchanged);
        assert!(model.bound().is_none());

        assert_eq!(
            seed_model(&mut model, vec![resource(2), other(1)]),
            SeedOutcome::Ambiguous(2)
        );
        assert!(model.bound().is_none(), "never pick by order");

        assert_eq!(
            seed_model(&mut model, vec![resource(2)]),
            SeedOutcome::Applied
        );
        assert_eq!(bound_id(&model), Some(("resource-5".to_string(), 2)));
        // The seeded model is the same value a bus frame would have produced.
        let mut via_bus = BrowserViewerModel::default();
        via_bus.bind_resource(resource(2)).unwrap();
        assert_eq!(model, via_bus);
    }

    #[test]
    fn seed_on_a_bound_model_only_advances_its_own_resource() {
        let mut model = BrowserViewerModel::default();
        model.bind_resource(resource(3)).unwrap();
        // Another resource never replaces the bound one; absence never unbinds.
        assert_eq!(
            seed_model(&mut model, vec![other(9)]),
            SeedOutcome::Unchanged
        );
        assert_eq!(seed_model(&mut model, vec![]), SeedOutcome::Unchanged);
        // Same revision: idempotent. Older: refused by the model, unchanged.
        assert_eq!(
            seed_model(&mut model, vec![resource(3)]),
            SeedOutcome::Unchanged
        );
        assert_eq!(
            seed_model(&mut model, vec![resource(2)]),
            SeedOutcome::Rejected(ModelError::StaleRevision)
        );
        assert_eq!(bound_id(&model), Some(("resource-5".to_string(), 3)));
        // Newer revision of the same id, picked out of several: refreshed.
        assert_eq!(
            seed_model(&mut model, vec![other(1), resource(4)]),
            SeedOutcome::Applied
        );
        assert_eq!(bound_id(&model), Some(("resource-5".to_string(), 4)));
    }

    #[test]
    fn seed_never_resurrects_a_closed_resource() {
        let mut model = BrowserViewerModel::default();
        model.bind_resource(resource(2)).unwrap();
        model.apply_closed(&closed("target-9")).unwrap();
        let before = model.clone();
        assert_eq!(
            seed_model(&mut model, vec![resource(2)]),
            SeedOutcome::Rejected(ModelError::AlreadyClosed)
        );
        assert_eq!(model, before);
    }

    #[test]
    fn seed_workspace_discards_a_snapshot_overtaken_by_a_bus_frame() {
        let workspace = BrowserWorkspace::new();
        let event = |topic: &str, data: Value| GatewayEvent {
            topic: topic.to_string(),
            data,
        };
        // Request sent; a close frame lands before the answer (on an unbound
        // model it leaves no tombstone): the snapshot must be discarded.
        let at_request = workspace.frame_seq();
        let _ = apply_browser_event(&event(CLOSED_TOPIC, closed_data("target-9")), &workspace);
        assert_eq!(
            seed_workspace(&workspace, at_request, vec![resource(2)]),
            SeedOutcome::Stale
        );
        assert!(
            !bound(&workspace),
            "a closed resource must not be resurrected"
        );

        // Undecodable traffic does not count as a frame.
        let at_request = workspace.frame_seq();
        let _ = apply_browser_event(&event("artifact.changed", json!({})), &workspace);
        assert_eq!(
            seed_workspace(&workspace, at_request, vec![resource(2)]),
            SeedOutcome::Applied
        );
        assert!(bound(&workspace));
    }

    #[test]
    fn workspace_binds_only_through_the_bridge_on_the_exact_topic() {
        let workspace = BrowserWorkspace::new();
        assert!(!bound(&workspace));

        let event = |topic: &str, data: Value| GatewayEvent {
            topic: topic.to_string(),
            data,
        };

        // Wrong topic / top-level payload: ignored, still unbound.
        assert_eq!(
            apply_browser_event(&event("browser.live.x", resource_data(2)), &workspace),
            BrowserEventOutcome::Ignored
        );
        assert_eq!(
            apply_browser_event(
                &event(RESOURCE_TOPIC, serde_json::to_value(resource(2)).unwrap()),
                &workspace
            ),
            BrowserEventOutcome::Ignored
        );
        assert!(!bound(&workspace));

        // Legitimate resource: bound.
        assert_eq!(
            apply_browser_event(&event(RESOURCE_TOPIC, resource_data(2)), &workspace),
            BrowserEventOutcome::Applied
        );
        assert!(bound(&workspace));

        // Stale revision is refused by the model and leaves it bound.
        assert!(matches!(
            apply_browser_event(&event(RESOURCE_TOPIC, resource_data(1)), &workspace),
            BrowserEventOutcome::Rejected(_)
        ));
        assert!(bound(&workspace));

        // Close for another target is refused; the matching one unbinds.
        assert!(matches!(
            apply_browser_event(&event(CLOSED_TOPIC, closed_data("other")), &workspace),
            BrowserEventOutcome::Rejected(_)
        ));
        assert!(bound(&workspace));
        assert_eq!(
            apply_browser_event(&event(CLOSED_TOPIC, closed_data("target-9")), &workspace),
            BrowserEventOutcome::Applied
        );
        assert!(!bound(&workspace));
    }
}
