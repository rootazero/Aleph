//! End-to-end scenarios driven through `FakeWaRuntime`.
//!
//! These four scenarios are the contract that Task 8 adds. They were
//! impossible to write before R1: the only test path was the
//! `test_mode: bool` shortcut in `WhatsAppChannel`, which short-circuited
//! every method but never exercised the event loop. With `FakeWaRuntime`
//! behind `Arc<dyn WaRuntime>`, the channel runs its real event loop in
//! tests, and tests can drive pairing state and inbound messages through
//! the same seam the production transport uses.
//!
//! ## Synchronisation
//!
//! `FakeWaRuntime::emit_*` pushes a `WaEvent` into the runtime's internal
//! mpsc and returns immediately. The event loop running inside
//! `WhatsAppChannel::start()` is on a separate tokio task and may take a
//! few millis to drain the channel. Each scenario waits on
//! `wait_for_status(...)`, which polls `Channel::status()` (a sync
//! `try_read` over the same `pairing_state` field the event loop writes)
//! until the expected status is observed or the timeout expires.

use std::time::Duration;

use alephcore::gateway::channel::{Channel, ChannelStatus};
use alephcore::gateway::interfaces::whatsapp::pairing::PairingState;
use alephcore::gateway::interfaces::whatsapp::wa_runtime::fake::FakeWaRuntime;
use alephcore::gateway::interfaces::whatsapp::WhatsAppChannel;

/// Poll `Channel::status()` until it equals `expected` or `timeout`
/// elapses. Polling is the only safe alternative to fixed `sleep(...)`
/// calls: a `sleep` that is too short flakes on slow CI, and a `sleep`
/// that is too long wastes seconds in the hot path.
async fn wait_for_status(channel: &WhatsAppChannel, expected: ChannelStatus, timeout: Duration) {
    let start = std::time::Instant::now();
    loop {
        if channel.status() == expected {
            return;
        }
        if start.elapsed() > timeout {
            panic!(
                "channel.status() did not reach {expected:?} within {timeout:?}; \
                 last seen {:?}",
                channel.status()
            );
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Reset both the fake's internal pairing state and the channel's
/// pairing-state mirror to `Idle`. The fake's state is what
/// `FakeWaRuntime::pairing_phase()` returns; the channel's is what
/// `Channel::status()` reads. Both must agree for the scenario's
/// observations to be meaningful.
async fn reset_pairing_state(channel: &WhatsAppChannel, fake: &FakeWaRuntime) {
    fake.set_pairing_state(PairingState::Idle).await;
    channel
        .reset_pairing_state_for_test(PairingState::Idle)
        .await;
}

/// Scenario 1 — `PairingQrCode` drives PairingState to `WaitingQr`, and
/// the channel's `status()` reflects the change.
///
/// This is the case the `test_mode` shortcut never covered: the event
/// loop in `WhatsAppChannel` must run for at least one iteration, the
/// `PairingStateDriver::apply()` arm for `PairingQrCode` must fire, and
/// `PairingState::to_channel_status()` must return `Connecting` for
/// `WaitingQr`. With `FakeWaRuntime`, all three happen on the test
/// machine without any network I/O.
#[tokio::test]
async fn scenario_qr_emitted_drives_pairing_state() {
    let (mut channel, fake) = WhatsAppChannel::for_test_with_fake("wa-test", Default::default());
    reset_pairing_state(&channel, &fake).await;

    channel
        .start()
        .await
        .expect("start should succeed in fake mode");

    // Drive a QR code: channel.pairing_state Idle → WaitingQr.
    fake.emit_qr("qr-data-1234").await.expect("emit_qr");
    wait_for_status(&channel, ChannelStatus::Connecting, Duration::from_secs(2)).await;

    assert_eq!(channel.status(), ChannelStatus::Connecting);
}

/// Scenario 2 — `PairSuccess` drives PairingState to `Connected` and
/// `Channel::status()` reflects that.
///
/// This is the second arm of `PairingStateDriver::apply()`. Pre-R1 the
/// `test_mode` path set `Connected` synchronously inside `start()` so the
/// status assertion was always trivially true. With the fake runtime, the
/// status flip must be driven by an emitted event, which is the same
/// shape the production transport drives it through.
#[tokio::test]
async fn scenario_pair_success_marks_connected() {
    let (mut channel, fake) = WhatsAppChannel::for_test_with_fake("wa-test", Default::default());
    reset_pairing_state(&channel, &fake).await;

    channel
        .start()
        .await
        .expect("start should succeed in fake mode");

    fake.emit_pair_success().await.expect("emit_pair_success");
    wait_for_status(&channel, ChannelStatus::Connected, Duration::from_secs(2)).await;

    assert_eq!(channel.status(), ChannelStatus::Connected);
}

/// Scenario 3 — An inbound `Event::Message` reaches the channel's
/// inbound broadcast, where `Channel::inbound_subscribe()` consumers
/// can pick it up.
///
/// The fake's `emit_text_message(...)` constructs an `Event::Message`
/// with a minimal `MessageInfo` and pushes it through the runtime's
/// event channel. The channel's event loop runs it through the inbound
/// mapper, which produces an `InboundMessage` and pushes it onto the
/// `ChannelState` broadcast. A subscriber on `inbound_subscribe()` then
/// receives it.
///
/// This is the scenario that proves the whole stack wires together: fake
/// → event loop → mapper → inbound broadcast → subscriber.
#[tokio::test]
async fn scenario_inbound_message_propagates_to_channel() {
    let (mut channel, fake) = WhatsAppChannel::for_test_with_fake("wa-test", Default::default());

    channel
        .start()
        .await
        .expect("start should succeed in fake mode");

    let mut rx = channel.inbound_subscribe();
    fake.emit_text_message("1234@s.whatsapp.net", "alice", "hello from fake")
        .await
        .expect("emit_text_message");

    let received = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("inbound message should arrive within timeout")
        .expect("subscriber should not be closed");

    assert_eq!(received.text, "hello from fake");
    assert_eq!(
        received.sender_id.as_str(),
        "alice@s.whatsapp.net",
        "sender id is the JID the mapper saw in Event::Message"
    );
    assert_eq!(received.conversation_id.as_str(), "1234@s.whatsapp.net");
}

/// Scenario 4 — After `Disconnected`, emitting `PairSuccess` again
/// re-establishes the connected state.
///
/// This is the reconnection path: spec §3.1 D1 says `PairingState::Disconnected`
/// may auto-reconnect to `Connected` directly via `Event::Connected`.
/// `PairingStateDriver` implements this by mapping `Event::Disconnected`
/// → `PairingState::Disconnected { .. }`, after which any
/// `Event::PairSuccess` (or `Event::Connected`) drives the state back to
/// `Connected`. The fake proves this loop without a real WhatsApp
/// reconnect.
#[tokio::test]
async fn scenario_reconnect_after_disconnect() {
    let (mut channel, fake) = WhatsAppChannel::for_test_with_fake("wa-test", Default::default());

    channel
        .start()
        .await
        .expect("start should succeed in fake mode");

    // Baseline: Connected (FakeWaRuntime defaults to Connected).
    assert_eq!(channel.status(), ChannelStatus::Connected);

    // Drop the connection: PairingState::Connected → Disconnected.
    fake.emit_disconnected("network blip")
        .await
        .expect("emit_disconnected");
    wait_for_status(
        &channel,
        ChannelStatus::Disconnected,
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(channel.status(), ChannelStatus::Disconnected);

    // Reconnect: emit PairSuccess again, status flips back to Connected.
    fake.emit_pair_success().await.expect("emit_pair_success");
    wait_for_status(&channel, ChannelStatus::Connected, Duration::from_secs(2)).await;
    assert_eq!(channel.status(), ChannelStatus::Connected);
}
