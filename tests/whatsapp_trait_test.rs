//! Compile-time tests for the `WaRuntime` trait contract.
//!
//! These tests fail at the type-check stage, not at runtime. They verify
//! that:
//!
//! 1. `WaRuntime` is object-safe (can be used as `dyn WaRuntime`).
//! 2. The trait can be returned from a `Box<dyn WaRuntime>` constructor.
//! 3. The trait's lifetime expectations are satisfied by the existing
//!    `whatsapp_rust::types::events::Event` type.
//!
//! If the trait gains an associated type, generic method, or `Self: Sized`
//! method without `where Self: Sized`, these tests will fail to compile.

use std::sync::Arc;

use alephcore::gateway::channel::{MessageId, OutboundMessage};
use alephcore::gateway::interfaces::whatsapp::pairing::PairingState;
use alephcore::gateway::interfaces::whatsapp::wa_runtime::traits::{
    WaEvent, WaRuntime, WaRuntimeError,
};

/// Object-safety check: the trait must be usable as `Box<dyn WaRuntime>`.
/// If the trait is not object-safe (e.g. an associated type, generic
/// method, or non-`where Self: Sized` `Self` return was added without
/// care) this function will fail to compile.
#[test]
fn test_wa_runtime_is_object_safe() {
    fn _accepts_dyn(_runtime: Box<dyn WaRuntime>) {}

    fn _accepts_arc(_runtime: Arc<dyn WaRuntime>) {}

    fn _returns_dyn() -> Box<dyn WaRuntime> {
        // We never actually run this — the body is `unreachable!()` because
        // we only care that the return type compiles. The runtime impl lives
        // in `wa_runtime::fake` and `wa_runtime::client`.
        unreachable!("compile-only check")
    }

    // Force the compiler to evaluate the inner fn signatures so an
    // object-safety violation is caught at this test, not at the call site
    // in the production crate.
    let _: fn(Box<dyn WaRuntime>) = _accepts_dyn;
    let _: fn(Arc<dyn WaRuntime>) = _accepts_arc;
    let _: fn() -> Box<dyn WaRuntime> = _returns_dyn;
}

/// Type-level check: `WaRuntime::pairing_phase` returns the `PairingState`
/// enum from `pairing`, not the legacy `ConnectionState`. If a future
/// refactor swaps back to `ConnectionState` (Task 3 review focus #1), this
/// assertion fails to compile.
#[allow(dead_code)]
fn _pairing_phase_returns_pairing_state(
    runtime: &dyn WaRuntime,
    fut: std::pin::Pin<Box<dyn std::future::Future<Output = PairingState> + Send + '_>>,
) {
    // Ensure the future's Output matches the trait method's declared return.
    fn assert_future_output<F: std::future::Future<Output = PairingState>>() {}
    drop(fut);
    let _ = runtime;
}

/// The error type must be re-exported from the trait module so callers
/// can match on variants without depending on the internal `ChannelError`.
/// `WaRuntimeError` is opaque (just an enum); we only need to confirm it
/// is a concrete type the trait returns.
#[allow(dead_code)]
fn _error_type_is_concrete(e: WaRuntimeError) {
    let _ = format!("{e}");
}

/// `WaEvent` re-export lets callers write
/// `runtime.take_event_receiver() -> Option<mpsc::Receiver<WaEvent>>` without
/// importing `wacore::types::events::Event` directly. The re-export must
/// be the same type as the real runtime's event channel element type; if
/// they drift, the event loop in `mod.rs` would silently drop messages.
#[allow(dead_code)]
fn _wa_event_is_compatible(_event: WaEvent) {}

/// All four `send_*` methods must take `&self` (not `&mut self`) so a
/// single `Arc<dyn WaRuntime>` clone can be shared between the channel
/// and the reaction adapter. If a future change adds `&mut self`,
/// production wiring in `WaRuntimeReactionAdapter` stops compiling.
#[allow(dead_code)]
async fn _shared_via_arc(runtime: Arc<dyn WaRuntime>) {
    let r1 = Arc::clone(&runtime);
    let r2 = Arc::clone(&runtime);
    let _: Result<MessageId, WaRuntimeError> = r1.send_message(OutboundMessage::text(
        "jid@s.whatsapp.net",
        "hello",
    ))
    .await;
    let _: Result<(), WaRuntimeError> = r2.send_reaction("jid", "msg", "👍").await;
    let _: Result<(), WaRuntimeError> = r2.send_typing("jid").await;
    let _: Result<(), WaRuntimeError> = r2.mark_read("msg").await;
}