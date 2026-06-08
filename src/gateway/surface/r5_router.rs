//! Core R5 router.
//!
//! Applies the "is this worth interrupting the user" policy ONCE, in the core,
//! and hands the result to every registered delivery surface. This is the
//! policy that used to live in the desktop shell's `notify.rs` (topic
//! membership + the long-run threshold). The shell now only focus-gates and
//! renders. The decisive focus-gate stays in the shell — the core cannot know
//! window focus.
//!
//! Scope: Phase 1 routes `AskUser` + `RunComplete`. `approval.requested` stays
//! on its existing operator-gated topic path until Phase 2 ("审批回投"), so a
//! guest surface never starts seeing approvals it could not see before.

use std::sync::Arc;

use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;

use super::delivery::{OutboundInteraction, SurfaceNotification, SurfaceRegistry};

/// Minimum turn duration before a completed run is worth interrupting for.
/// Mirrors the shell's former `COMPLETION_NOTIFY_MIN_MS` — the policy moved here.
const COMPLETION_NOTIFY_MIN_MS: u64 = 15_000;

/// Max banner body length (char-boundary safe). Mirrors the shell's former cap.
const MAX_BODY_CHARS: usize = 180;

/// Pure policy: map an R5-relevant frame to the notification the user should be
/// interrupted with, or `None` to stay silent. Focus-gating is NOT here (only
/// the shell knows focus); this decides interrupt-worthiness + content only.
pub fn notification_for(frame: &GatewayEventFrame) -> Option<SurfaceNotification> {
    match frame {
        GatewayEventFrame::AskUser { question, .. } => {
            let q = question.trim();
            Some(SurfaceNotification {
                title: "Aleph has a question".to_string(),
                body: if q.is_empty() {
                    "Aleph is waiting for your reply.".to_string()
                } else {
                    truncate(q, MAX_BODY_CHARS)
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
        // Everything else stays silent — including `SurfaceNotify` itself.
        // LOOP-SAFETY: `DesktopSurface::deliver` publishes `SurfaceNotify` back
        // onto the same bus this router subscribes to. It MUST fall through to
        // `None` here; adding a `SurfaceNotify` arm would re-deliver our own
        // output and amplify infinitely. `ApprovalRequested` also stays here
        // (operator-gated path until Phase 2).
        _ => None,
    }
}

/// Truncate to `max` chars on a char boundary, appending an ellipsis.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
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
                        if let Err(e) =
                            surface.deliver(OutboundInteraction::Notify(note.clone()))
                        {
                            tracing::debug!(error = %e, "surface delivery failed");
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
        let n = notification_for(&run_complete(COMPLETION_NOTIFY_MIN_MS)).expect("long run notifies");
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
        };
        assert!(notification_for(&f).is_none());
    }

    #[tokio::test]
    async fn run_delivers_to_registered_surface() {
        use crate::gateway::surface::delivery::{DeliveryError, DeliverySurface};
        use crate::gateway::surface::SurfaceKind;
        use std::sync::Mutex;

        struct Capture(Arc<Mutex<Vec<SurfaceNotification>>>);
        impl DeliverySurface for Capture {
            fn kind(&self) -> SurfaceKind {
                SurfaceKind::Desktop
            }
            fn deliver(&self, outbound: OutboundInteraction) -> Result<(), DeliveryError> {
                let OutboundInteraction::Notify(n) = outbound;
                self.0.lock().unwrap().push(n);
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
