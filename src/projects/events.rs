//! Event-bus publisher for `projects.changed` (Task 6, P5 ruling).
//!
//! [`publish_changed`] is the SINGLE publisher — every `projects.*` mutation
//! handler that commits a change calls this rather than constructing
//! [`GatewayEventFrame::ProjectsChanged`] inline at its own call site. A
//! later task (`project_manage`, a model-facing tool over the same store)
//! also has to emit this frame; sharing one function from the start is what
//! keeps that task from copying the frame construction a second time. Mirrors
//! `gateway::handlers::teams::crud::notify_team_changed`.

use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::{ChangeKind, GatewayEventFrame};

/// Emit a `projects.changed` frame so every subscribed surface re-fetches:
/// the sidebar's project list (`projects.list`) and an open room's page
/// (`projects.get` + its roster) both listen on this topic. Without it the
/// two views drift apart — or, for a member who was just added or removed,
/// never learn about the room at all — until a manual reload.
///
/// Best-effort — a serialization failure must not fail the RPC that
/// triggered the mutation; see `GatewayEventBus::publish_frame`'s own
/// contract, which every other `notify_*_changed` helper in this crate
/// relies on the same way.
///
/// `affected_user` is set ONLY by the `member_remove` mutation, naming the
/// user who was just dropped from the roster — see
/// `GatewayEventFrame::ProjectsChanged::affected_user`'s doc for why that one
/// frame needs a carve-out past the ordinary roster-membership visibility
/// rule. Every other mutation passes `None`.
pub fn publish_changed(
    event_bus: &GatewayEventBus,
    project_id: &str,
    change: ChangeKind,
    affected_user: Option<&str>,
) {
    let _ = event_bus.publish_frame(&GatewayEventFrame::ProjectsChanged {
        project_id: project_id.to_string(),
        change,
        affected_user: affected_user.map(str::to_string),
    });
}
