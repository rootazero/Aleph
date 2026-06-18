//! Standalone bootstrap factory helpers extracted from `start/mod.rs`.
//!
//! These are self-contained free functions with explicit signatures and no
//! coupling to the `start_server` body's locals — the only cleanly extractable
//! seams in an otherwise monolithic bootstrap sequence.

use std::sync::Arc;

/// Build the shared task-result delivery engine with all built-in targets
/// (Webhook / Gateway / Memory) registered.
///
/// Used by **both** the cron and heartbeat timer loops so every task type
/// resolves the same target set — previously each subsystem registered the
/// targets inline, and the cron alert path skipped the engine entirely
/// (silently dropping `Webhook` / `Memory` failure-alert targets). Centralising
/// it here keeps the registration single-source.
pub(super) fn build_task_delivery_engine(
    channel_cell: alephcore::tasks::shared::targets::ChannelRegistryCell,
    memory_store: Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>,
    ssrf_policy: alephcore::security::ssrf::SsrfPolicy,
) -> Arc<alephcore::tasks::shared::delivery::DeliveryEngine> {
    use alephcore::tasks::shared::delivery::DeliveryEngine;
    use alephcore::tasks::shared::targets::{GatewayDeliveryTarget, MemoryDeliveryTarget};

    let mut engine = DeliveryEngine::new();
    engine.register(Arc::new(
        alephcore::tasks::cron::webhook_target::WebhookTarget::new(ssrf_policy),
    ));
    engine.register(Arc::new(GatewayDeliveryTarget::new(channel_cell)));
    engine.register(Arc::new(MemoryDeliveryTarget::new(memory_store)));
    Arc::new(engine)
}

/// Build the platform-specific `DesktopPlatform` instance.
pub(super) fn build_desktop_platform() -> Arc<dyn aleph_desktop::DesktopPlatform> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(aleph_desktop_macos::MacOSPlatform::new())
    }
    #[cfg(target_os = "linux")]
    {
        Arc::new(aleph_desktop_linux::LinuxPlatform::new())
    }
    #[cfg(target_os = "windows")]
    {
        Arc::new(aleph_desktop_windows::WindowsPlatform::new())
    }
}
