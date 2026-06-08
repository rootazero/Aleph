//! The desktop shell as a first-class delivery surface.
//!
//! R1 boundary holds: this only publishes a `SurfaceNotify` frame addressed to
//! `desktop` connections; the shell's `notify.rs` renders the OS banner and
//! owns the focus-gate. No rendering happens in core.

use std::sync::Arc;

use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;

use super::delivery::{DeliveryError, DeliverySurface, OutboundInteraction};
use super::SurfaceKind;

pub struct DesktopSurface {
    event_bus: Arc<GatewayEventBus>,
}

impl DesktopSurface {
    pub fn new(event_bus: Arc<GatewayEventBus>) -> Self {
        Self { event_bus }
    }
}

impl DeliverySurface for DesktopSurface {
    fn kind(&self) -> SurfaceKind {
        SurfaceKind::Desktop
    }

    fn deliver(&self, outbound: OutboundInteraction) -> Result<(), DeliveryError> {
        let OutboundInteraction::Notify(n) = outbound;
        self.event_bus
            .publish_frame(&GatewayEventFrame::SurfaceNotify {
                audience: vec![SurfaceKind::Desktop.as_str().to_string()],
                title: n.title,
                body: n.body,
                source_topic: n.source_topic,
            })
            .map(|_| ())
            .map_err(|e| DeliveryError::Publish(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::surface::delivery::SurfaceNotification;

    #[tokio::test]
    async fn deliver_publishes_surface_notify_to_desktop_audience() {
        let bus = Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe_typed();
        let surface = DesktopSurface::new(bus.clone());

        assert_eq!(surface.kind(), SurfaceKind::Desktop);

        surface
            .deliver(OutboundInteraction::Notify(SurfaceNotification {
                title: "Aleph finished".to_string(),
                body: "Your turn is complete.".to_string(),
                source_topic: "agent.run.complete".to_string(),
            }))
            .unwrap();

        match rx.recv().await.unwrap() {
            GatewayEventFrame::SurfaceNotify {
                audience,
                title,
                source_topic,
                ..
            } => {
                assert_eq!(audience, vec!["desktop".to_string()]);
                assert_eq!(title, "Aleph finished");
                assert_eq!(source_topic, "agent.run.complete");
            }
            other => panic!("expected SurfaceNotify, got {other:?}"),
        }
    }
}
