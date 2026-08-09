//! Core R5 router.
//!
//! Applies the "is this worth interrupting the user" policy ONCE, in the core,
//! and hands the result to every registered delivery surface. This is the
//! policy that used to live in the desktop shell's `notify.rs` (topic
//! membership + the long-run threshold). The shell now only focus-gates and
//! renders. The decisive focus-gate stays in the shell — the core cannot know
//! window focus.
//!
//! Scope: Phase 1 routes `AskUser` + `RunComplete` (via `notification_for`).
//! Phase 2 additionally routes `ApprovalRequested` (via `approval_for`) to a
//! gated `surface.approval` frame; the raw `approval.requested` frame still
//! drives the Panel card and the inbound `manager.resolve` correlation.

use crate::sync_primitives::Arc;

use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;
use crate::utils::text_format::truncate_with_marker;

use super::delivery::{OutboundInteraction, SurfaceApproval, SurfaceNotification, SurfaceRegistry};

/// Minimum turn duration before a completed run is worth interrupting for.
/// Mirrors the shell's former `COMPLETION_NOTIFY_MIN_MS` — the policy moved here.
const COMPLETION_NOTIFY_MIN_MS: u64 = 15_000;

/// Max banner body length (char-boundary safe). Mirrors the shell's former cap.
const MAX_BODY_CHARS: usize = 180;

/// Pure policy: map an R5-relevant frame to the notification the user should be
/// interrupted with, or `None` to stay silent. Focus-gating is NOT here (only
/// the shell knows focus); this decides interrupt-worthiness + content only.
#[must_use]
pub fn notification_for(frame: &GatewayEventFrame) -> Option<SurfaceNotification> {
    match frame {
        GatewayEventFrame::AskUser { question, .. } => {
            let q = question.trim();
            Some(SurfaceNotification {
                title: "Aleph has a question".to_string(),
                body: if q.is_empty() {
                    "Aleph is waiting for your reply.".to_string()
                } else {
                    truncate_with_marker(q, MAX_BODY_CHARS, "…")
                },
                source_topic: frame.topic_name(),
            })
        }
        GatewayEventFrame::RunComplete {
            total_duration_ms, ..
        } => {
            if *total_duration_ms < COMPLETION_NOTIFY_MIN_MS {
                return None;
            }
            Some(SurfaceNotification {
                title: "Aleph finished".to_string(),
                body: "Your turn is complete.".to_string(),
                source_topic: frame.topic_name(),
            })
        }
        // Everything else stays silent for *notifications* — including both
        // surface frames (loop-safety) and `ApprovalRequested` (handled by
        // `approval_for`, which delivers an `ApprovalRequest`, not a `Notify`).
        _ => None,
    }
}

/// Pure policy: map an `ApprovalRequested` frame to the approval banner the
/// operator should be interrupted with. The raw frame is sparse (`approval_id`
/// only) — detail lives in the Panel card; the banner carries static text.
///
/// Returns `None` for everything else, INCLUDING `SurfaceApproval` itself.
/// LOOP-SAFETY: `DesktopSurface::deliver` publishes `SurfaceApproval` back onto
/// the same bus this router subscribes to; it MUST fall through to `None` here
/// or we re-deliver our own output and amplify infinitely. The ownership gate is
/// applied later, at the forward-filter (`event_visibility` +
/// `audience_allows`), not here.
///
/// `session_key` is carried through verbatim — including the empty string a
/// cluster-node approval arrives with. It is the banner's only routing
/// information: dropping it here is what used to force the frame to be
/// `Global` + role-gated downstream, i.e. delivered to operators only.
#[must_use]
pub fn approval_for(frame: &GatewayEventFrame) -> Option<SurfaceApproval> {
    match frame {
        GatewayEventFrame::ApprovalRequested {
            approval_id,
            session_key,
            ..
        } => Some(SurfaceApproval {
            approval_id: approval_id.clone(),
            session_key: session_key.clone(),
            title: "Aleph needs your approval".to_string(),
            body: "A tool call is waiting for you.".to_string(),
        }),
        _ => None,
    }
}

/// Subscribe the typed bus and route interrupt-worthy R5 frames to every
/// registered surface. Runs until the bus closes. A `Lagged` receiver simply
/// skips dropped frames (R5 is best-effort; a missed banner is not fatal).
pub async fn run(event_bus: Arc<GatewayEventBus>, surfaces: SurfaceRegistry) {
    let mut rx = event_bus.subscribe_typed();
    loop {
        match rx.recv().await {
            Ok(frame) => {
                if let Some(note) = notification_for(&frame) {
                    for surface in &surfaces {
                        if let Err(e) = surface.deliver(OutboundInteraction::Notify(note.clone())) {
                            tracing::debug!(error = %e, "surface delivery failed");
                        }
                    }
                }
                if let Some(approval) = approval_for(&frame) {
                    for surface in &surfaces {
                        if let Err(e) =
                            surface.deliver(OutboundInteraction::ApprovalRequest(approval.clone()))
                        {
                            tracing::debug!(error = %e, "surface approval delivery failed");
                        }
                    }
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_emitter::RunSummary;

    fn run_complete(ms: u64) -> GatewayEventFrame {
        GatewayEventFrame::RunComplete {
            run_id: "r1".to_string(),
            seq: 1,
            summary: RunSummary::default(),
            total_duration_ms: ms,
        }
    }

    #[test]
    fn ask_user_surfaces_the_question() {
        let f = GatewayEventFrame::AskUser {
            run_id: "r1".to_string(),
            seq: 1,
            session_key: "sess-1".to_string(),
            question: "Delete it?".to_string(),
            options: vec![],
        };
        let n = notification_for(&f).expect("ask_user notifies");
        assert_eq!(n.title, "Aleph has a question");
        assert_eq!(n.body, "Delete it?");
        assert_eq!(n.source_topic, "agent.ask.user");
    }

    #[test]
    fn empty_question_falls_back() {
        let f = GatewayEventFrame::AskUser {
            run_id: "r1".to_string(),
            seq: 1,
            session_key: "sess-1".to_string(),
            question: "   ".to_string(),
            options: vec![],
        };
        assert_eq!(
            notification_for(&f).unwrap().body,
            "Aleph is waiting for your reply."
        );
    }

    #[test]
    fn run_complete_is_gated_by_duration() {
        assert!(notification_for(&run_complete(COMPLETION_NOTIFY_MIN_MS - 1)).is_none());
        let n =
            notification_for(&run_complete(COMPLETION_NOTIFY_MIN_MS)).expect("long run notifies");
        assert_eq!(n.title, "Aleph finished");
        assert_eq!(n.body, "Your turn is complete.");
        assert_eq!(n.source_topic, "agent.run.complete");
    }

    #[test]
    fn approval_is_not_routed_here() {
        let f = GatewayEventFrame::ApprovalRequested {
            approval_id: "a1".to_string(),
            session_key: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            tool_call_id: None,
        };
        assert!(notification_for(&f).is_none());
    }

    #[test]
    fn approval_for_surfaces_the_request() {
        let f = GatewayEventFrame::ApprovalRequested {
            approval_id: "a1".to_string(),
            session_key: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            tool_call_id: None,
        };
        let a = approval_for(&f).expect("approval is surfaced");
        assert_eq!(a.approval_id, "a1");
        assert_eq!(a.title, "Aleph needs your approval");
        assert_eq!(a.body, "A tool call is waiting for you.");
    }

    /// The banner's routing information. Dropping it here is what pinned the
    /// frame to `Global` downstream, so it is asserted at the point of copy —
    /// including the fleet case, where the empty string is the answer and must
    /// not be turned into anything else.
    #[test]
    fn approval_for_carries_the_session_key_through() {
        let owned = GatewayEventFrame::ApprovalRequested {
            approval_id: "a1".to_string(),
            session_key: "agent:main:s1".to_string(),
            channel_id: String::new(),
            conversation_id: String::new(),
            tool_call_id: None,
        };
        assert_eq!(
            approval_for(&owned).expect("surfaced").session_key,
            "agent:main:s1"
        );

        let fleet = GatewayEventFrame::ApprovalRequested {
            approval_id: "a2".to_string(),
            session_key: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            tool_call_id: None,
        };
        assert_eq!(
            approval_for(&fleet).expect("surfaced").session_key,
            "",
            "a cluster-node approval has no session and must stay that way — \
             the empty string is what makes it operator-only downstream"
        );
    }

    #[test]
    fn approval_for_ignores_its_own_surface_frame() {
        // LOOP-SAFETY: SurfaceApproval is this router's own output.
        let f = GatewayEventFrame::SurfaceApproval {
            audience: vec!["desktop".to_string()],
            approval_id: "a1".to_string(),
            session_key: "agent:main:s1".to_string(),
            title: "x".to_string(),
            body: "y".to_string(),
        };
        assert!(approval_for(&f).is_none());
    }

    #[test]
    fn approval_for_ignores_ask_user() {
        let f = GatewayEventFrame::AskUser {
            run_id: "r1".to_string(),
            seq: 1,
            session_key: "sess-1".to_string(),
            question: "Proceed?".to_string(),
            options: vec![],
        };
        assert!(approval_for(&f).is_none());
    }

    #[tokio::test]
    async fn run_delivers_approval_to_registered_surface() {
        use crate::gateway::surface::delivery::{DeliveryError, DeliverySurface, SurfaceApproval};
        use crate::gateway::surface::SurfaceKind;
        use crate::sync_primitives::Mutex;

        struct ApprovalCapture(Arc<Mutex<Vec<SurfaceApproval>>>);
        impl DeliverySurface for ApprovalCapture {
            fn kind(&self) -> SurfaceKind {
                SurfaceKind::Desktop
            }
            fn deliver(&self, outbound: OutboundInteraction) -> Result<(), DeliveryError> {
                if let OutboundInteraction::ApprovalRequest(a) = outbound {
                    self.0.lock().unwrap().push(a);
                }
                Ok(())
            }
        }

        let bus = Arc::new(GatewayEventBus::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let surfaces: SurfaceRegistry = vec![Arc::new(ApprovalCapture(seen.clone()))];
        let task = tokio::spawn(run(bus.clone(), surfaces));

        tokio::task::yield_now().await;
        let _ = bus.publish_frame(&GatewayEventFrame::ApprovalRequested {
            approval_id: "a1".to_string(),
            session_key: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            tool_call_id: None,
        });

        for _ in 0..50 {
            if !seen.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        task.abort();

        let got = seen.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].approval_id, "a1");
        assert_eq!(got[0].title, "Aleph needs your approval");
    }

    #[tokio::test]
    async fn run_delivers_to_registered_surface() {
        use crate::gateway::surface::delivery::{DeliveryError, DeliverySurface};
        use crate::gateway::surface::SurfaceKind;
        use crate::sync_primitives::Mutex;

        struct Capture(Arc<Mutex<Vec<SurfaceNotification>>>);
        impl DeliverySurface for Capture {
            fn kind(&self) -> SurfaceKind {
                SurfaceKind::Desktop
            }
            fn deliver(&self, outbound: OutboundInteraction) -> Result<(), DeliveryError> {
                match outbound {
                    OutboundInteraction::Notify(n) => self.0.lock().unwrap().push(n),
                    OutboundInteraction::ApprovalRequest(_) => {}
                }
                Ok(())
            }
        }

        let bus = Arc::new(GatewayEventBus::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let surfaces: SurfaceRegistry = vec![Arc::new(Capture(seen.clone()))];
        let task = tokio::spawn(run(bus.clone(), surfaces));

        tokio::task::yield_now().await;
        let _ = bus.publish_frame(&run_complete(COMPLETION_NOTIFY_MIN_MS));

        for _ in 0..50 {
            if !seen.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        task.abort();

        let got = seen.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "Aleph finished");
    }
}
