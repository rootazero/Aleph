//! SMS is an `ExplicitOnly` outbound channel: the generic registry and durable
//! queue must neither retry it nor persist/replay it. These tests drive a real
//! `ChannelRegistry` + `DeliveryStore` + `SmsChannel`; the only fake is the
//! Relay transport (no network, no provider).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aleph_protocol::telecom_relay::TelecomOutcome;
use alephcore::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelId, ChannelInfo, ChannelResult,
    ChannelState, ChannelStatus, MessageId, OutboundDeliveryPolicy, OutboundMessage, SendOperation,
    SendResult,
};
use alephcore::gateway::channel_registry::ChannelRegistry;
use alephcore::gateway::delivery_queue::{
    enqueue_validated_legacy, now_secs, spawn_drain, DeliveryQueueConfig, DeliveryStore,
};
use alephcore::gateway::interfaces::sms::{
    PendingSmsOperation, SmsChannel, SmsConfig, SmsSubmission, SmsTransport,
};
use async_trait::async_trait;

const SMS_ID: &str = "sms-test";
const PEER: &str = "+14155550100";
const LEGACY_BODY: &str = "legacy-secret-body-7a1c";

fn sms_config() -> SmsConfig {
    serde_json::from_value(serde_json::json!({
        "region": "CN",
        "service_address": "+8613800000000",
        "binding_ref": "core-binding-1",
    }))
    .expect("valid sms config")
}

#[derive(Clone, Copy)]
enum Mode {
    Accept,
    RateLimited,
    Unknown,
}

struct FakeTransport {
    connected: AtomicBool,
    mode: Mode,
    submits: AtomicUsize,
}

impl FakeTransport {
    fn new(mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            connected: AtomicBool::new(true),
            mode,
            submits: AtomicUsize::new(0),
        })
    }

    fn submits(&self) -> usize {
        self.submits.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SmsTransport for FakeTransport {
    async fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn submit(&self, operation: &PendingSmsOperation) -> ChannelResult<SmsSubmission> {
        self.submits.fetch_add(1, Ordering::SeqCst);
        match self.mode {
            Mode::RateLimited => Err(ChannelError::RateLimited {
                retry_after_secs: 0,
            }),
            Mode::Accept => Ok(SmsSubmission {
                operation_id: operation.operation_id().to_string(),
                outcome: TelecomOutcome::Accepted,
            }),
            Mode::Unknown => Ok(SmsSubmission {
                operation_id: operation.operation_id().to_string(),
                outcome: TelecomOutcome::Unknown,
            }),
        }
    }
}

fn fast_cfg() -> DeliveryQueueConfig {
    DeliveryQueueConfig {
        tick: Duration::from_millis(40),
        ..DeliveryQueueConfig::default()
    }
}

struct Fixture {
    registry: Arc<ChannelRegistry>,
    store: Arc<DeliveryStore>,
    transport: Arc<FakeTransport>,
}

/// Registry with a real store and a started SMS channel on `mode`.
async fn sms_fixture(mode: Mode) -> Fixture {
    let store = Arc::new(DeliveryStore::open_in_memory(fast_cfg()).expect("store"));
    let registry = Arc::new(ChannelRegistry::new().with_delivery_store(Arc::clone(&store)));
    let transport = FakeTransport::new(mode);
    let channel = SmsChannel::new(SMS_ID, sms_config()).with_transport(transport.clone());
    let id = registry.register(Box::new(channel)).await;
    registry.start_channel(&id).await.expect("sms starts");
    Fixture {
        registry,
        store,
        transport,
    }
}

fn sms_id() -> ChannelId {
    ChannelId::new(SMS_ID)
}

fn assert_nothing_stored(store: &DeliveryStore) {
    assert_eq!(store.len().unwrap(), 0, "no live queue row");
    assert!(
        store.recent_dead_letters(10).unwrap().is_empty(),
        "no dead letter (it would carry the plaintext body)"
    );
}

/// Insert a row the way the pre-policy generic queue would have.
async fn seed_legacy_row(
    registry: &ChannelRegistry,
    store: &DeliveryStore,
    conversation: &str,
    body: &str,
) {
    enqueue_validated_legacy(
        registry,
        store,
        SMS_ID,
        &OutboundMessage::text(conversation, body),
        "NotConnected",
        now_secs() - 10,
    )
    .await
    .expect("seed legacy row");
}

fn assert_scrubbed_retirement(store: &DeliveryStore, expected_rows: usize) {
    assert_eq!(store.len().unwrap(), 0, "no row left in the live queue");
    let letters = store.recent_dead_letters(10).unwrap();
    assert_eq!(letters.len(), expected_rows, "{letters:?}");
    for l in &letters {
        assert_eq!(l.channel_id, SMS_ID);
        assert!(
            !format!("{l:?}").contains(LEGACY_BODY),
            "retired row still carries the plaintext body"
        );
        assert!(l.message.text.is_empty(), "body scrubbed");
        assert!(l.message.attachments.is_empty(), "attachments scrubbed");
        assert_eq!(l.reason.as_str(), "explicit_only_retired");
        assert!(
            !l.reason.redrivable(),
            "a policy retirement is not redrivable"
        );
        assert!(
            l.last_error.contains("explicit"),
            "truthful reason: {}",
            l.last_error
        );
    }
}

// ---- the policy is the channel's own declaration -----------------------------

#[test]
fn sms_declares_explicit_only_and_ordinary_channels_stay_automatic() {
    let sms = SmsChannel::new(SMS_ID, sms_config());
    assert_eq!(
        sms.outbound_delivery_policy(),
        OutboundDeliveryPolicy::ExplicitOnly
    );
    assert_eq!(
        MockChannel::new("m", Arc::default()).outbound_delivery_policy(),
        OutboundDeliveryPolicy::Automatic
    );
}

// ---- fresh sends ---------------------------------------------------------------

#[tokio::test]
async fn disconnected_fresh_sms_is_not_connected_and_stores_nothing() {
    let fx = sms_fixture(Mode::Accept).await;
    fx.transport.connected.store(false, Ordering::SeqCst);

    let err = fx
        .registry
        .send(&sms_id(), OutboundMessage::text(PEER, "fresh body"))
        .await
        .expect_err("relay is down");

    assert!(matches!(err, ChannelError::NotConnected(_)), "{err:?}");
    assert_eq!(fx.transport.submits(), 0);
    assert_nothing_stored(&fx.store);
}

#[tokio::test]
async fn rate_limited_sms_is_submitted_exactly_once_and_not_stored() {
    let fx = sms_fixture(Mode::RateLimited).await;

    let err = fx
        .registry
        .send(&sms_id(), OutboundMessage::text(PEER, "fresh body"))
        .await
        .expect_err("rate limited");

    assert!(matches!(err, ChannelError::RateLimited { .. }), "{err:?}");
    assert_eq!(
        fx.transport.submits(),
        1,
        "the registry's in-memory RateLimited retry must not re-submit SMS"
    );
    assert_nothing_stored(&fx.store);
}

#[tokio::test]
async fn unknown_sms_is_submitted_exactly_once_and_not_stored() {
    let fx = sms_fixture(Mode::Unknown).await;

    let err = fx
        .registry
        .send(&sms_id(), OutboundMessage::text(PEER, "fresh body"))
        .await
        .expect_err("unknown outcome");

    assert!(matches!(err, ChannelError::SendFailed(_)), "{err:?}");
    assert_eq!(fx.transport.submits(), 1);
    assert_nothing_stored(&fx.store);
}

#[tokio::test]
async fn a_live_sms_send_still_submits_once() {
    let fx = sms_fixture(Mode::Accept).await;
    fx.registry
        .send(&sms_id(), OutboundMessage::text(PEER, "fresh body"))
        .await
        .expect("accepted");
    assert_eq!(fx.transport.submits(), 1);
    assert_nothing_stored(&fx.store);
}

// ---- legacy stored rows ----------------------------------------------------------

#[tokio::test]
async fn preflush_retires_legacy_sms_rows_without_sending_and_the_new_send_goes_once() {
    let fx = sms_fixture(Mode::Accept).await;
    seed_legacy_row(&fx.registry, &fx.store, PEER, LEGACY_BODY).await;
    seed_legacy_row(&fx.registry, &fx.store, PEER, LEGACY_BODY).await;

    fx.registry
        .send(&sms_id(), OutboundMessage::text(PEER, "new explicit send"))
        .await
        .expect("new send accepted");

    assert_eq!(
        fx.transport.submits(),
        1,
        "only the newly requested SMS is submitted; legacy rows are never replayed"
    );
    assert_scrubbed_retirement(&fx.store, 2);
}

#[tokio::test]
async fn background_drain_retires_legacy_sms_rows_without_sending() {
    let fx = sms_fixture(Mode::Accept).await;
    seed_legacy_row(&fx.registry, &fx.store, PEER, LEGACY_BODY).await;
    seed_legacy_row(&fx.registry, &fx.store, "+14155550111", LEGACY_BODY).await;

    spawn_drain(Arc::clone(&fx.registry), Arc::clone(&fx.store));
    for _ in 0..100 {
        if fx.store.len().unwrap() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }

    assert_eq!(fx.transport.submits(), 0, "drain must never submit SMS");
    assert_scrubbed_retirement(&fx.store, 2);
}

#[tokio::test]
async fn a_failed_new_send_does_not_enqueue_behind_retired_legacy_rows() {
    let fx = sms_fixture(Mode::RateLimited).await;
    seed_legacy_row(&fx.registry, &fx.store, PEER, LEGACY_BODY).await;

    let _ = fx
        .registry
        .send(&sms_id(), OutboundMessage::text(PEER, "new body"))
        .await
        .expect_err("rate limited");

    assert_eq!(fx.transport.submits(), 1);
    assert_scrubbed_retirement(&fx.store, 1);
}

// ---- ordinary channels are unchanged ----------------------------------------------

#[derive(Default)]
struct Sends {
    attempts: AtomicUsize,
    stops: AtomicUsize,
    texts: Mutex<Vec<String>>,
}

#[derive(Clone, Copy)]
enum Fail {
    Never,
    NotConnected,
    RateLimitedThenOk,
}

struct MockChannel {
    info: ChannelInfo,
    state: ChannelState,
    sends: Arc<Sends>,
    fail: Fail,
}

impl MockChannel {
    fn new(id: &str, sends: Arc<Sends>) -> Self {
        Self::failing(id, sends, Fail::Never)
    }

    fn failing(id: &str, sends: Arc<Sends>, fail: Fail) -> Self {
        Self {
            info: ChannelInfo {
                id: ChannelId::new(id),
                name: id.to_string(),
                channel_type: "mock".to_string(),
                status: ChannelStatus::Connected,
                capabilities: ChannelCapabilities::default(),
            },
            state: ChannelState::new(8),
            sends,
            fail,
        }
    }
}

#[async_trait]
impl Channel for MockChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }
    fn state(&self) -> &ChannelState {
        &self.state
    }
    async fn start(&mut self) -> ChannelResult<()> {
        Ok(())
    }
    async fn stop(&mut self) -> ChannelResult<()> {
        Ok(())
    }
    fn send(&self, message: OutboundMessage) -> SendOperation {
        let n = self.sends.attempts.fetch_add(1, Ordering::SeqCst);
        let fail = self.fail;
        let sends = Arc::clone(&self.sends);
        Box::pin(async move {
            match fail {
                Fail::NotConnected => return Err(ChannelError::NotConnected("down".into())),
                Fail::RateLimitedThenOk if n == 0 => {
                    return Err(ChannelError::RateLimited {
                        retry_after_secs: 0,
                    })
                }
                _ => {}
            }
            sends.texts.lock().unwrap().push(message.text);
            Ok(SendResult {
                message_id: MessageId::new("ok"),
                timestamp: chrono::Utc::now(),
            })
        })
    }
}

#[tokio::test]
async fn automatic_channel_keeps_rate_limit_retry() {
    let store = Arc::new(DeliveryStore::open_in_memory(fast_cfg()).unwrap());
    let registry = ChannelRegistry::new().with_delivery_store(Arc::clone(&store));
    let sends = Arc::new(Sends::default());
    registry
        .register(Box::new(MockChannel::failing(
            "m",
            sends.clone(),
            Fail::RateLimitedThenOk,
        )))
        .await;

    registry
        .send(&ChannelId::new("m"), OutboundMessage::text("c", "hello"))
        .await
        .expect("retried then delivered");
    assert_eq!(sends.attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn automatic_channel_still_queues_and_drains_a_not_connected_send() {
    let store = Arc::new(DeliveryStore::open_in_memory(fast_cfg()).unwrap());
    let registry = Arc::new(ChannelRegistry::new().with_delivery_store(Arc::clone(&store)));
    let sends = Arc::new(Sends::default());
    registry
        .register(Box::new(MockChannel::failing(
            "m",
            sends.clone(),
            Fail::NotConnected,
        )))
        .await;

    let err = registry
        .send(
            &ChannelId::new("m"),
            OutboundMessage::text("c", "queued me"),
        )
        .await
        .expect_err("down");
    assert!(matches!(err, ChannelError::NotConnected(_)));
    assert_eq!(store.len().unwrap(), 1, "ordinary channel still queues");
    assert!(store.recent_dead_letters(10).unwrap().is_empty());

    // Replace with a healthy instance; the queued row must replay (drain unchanged).
    registry
        .register(Box::new(MockChannel::new("m", sends.clone())))
        .await;
    store.reschedule(1, 0, now_secs() - 1, "test").unwrap();
    spawn_drain(Arc::clone(&registry), Arc::clone(&store));
    for _ in 0..100 {
        if store.len().unwrap() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    assert_eq!(store.len().unwrap(), 0);
    assert_eq!(
        sends.texts.lock().unwrap().as_slice(),
        ["queued me".to_string()]
    );
}

#[tokio::test]
async fn queued_rows_for_an_unregistered_channel_keep_existing_behaviour() {
    // No policy marker is provable for a channel that is not registered: the
    // row must stay queued (rescheduled), exactly as before.
    let store = Arc::new(DeliveryStore::open_in_memory(fast_cfg()).unwrap());
    let registry = Arc::new(ChannelRegistry::new().with_delivery_store(Arc::clone(&store)));
    let sends = Arc::new(Sends::default());
    registry
        .register(Box::new(MockChannel::new("ghost", sends)))
        .await;
    enqueue_validated_legacy(
        &registry,
        &store,
        "ghost",
        &OutboundMessage::text("c", "keep me"),
        "NotConnected",
        now_secs() - 10,
    )
    .await
    .unwrap();
    registry.unregister(&ChannelId::new("ghost")).await;
    spawn_drain(Arc::clone(&registry), Arc::clone(&store));
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(store.len().unwrap(), 1, "still queued");
    assert!(store.recent_dead_letters(10).unwrap().is_empty());
}

// ======================= fix round 1 =============================================
//
// I1: the policy that governed the transport call is the one that governs the
// enqueue decision. I4: every retained, recognised ExplicitOnly payload is
// logically scrubbed and cannot be put back in the queue.

/// An `ExplicitOnly` channel whose `send` removes itself from the registry
/// (optionally putting an `Automatic` replacement in its place) and then fails
/// with a queueable `NotConnected` — the race a post-send policy re-lookup
/// reads as "no declaration, so enqueue".
struct VanishingExplicit {
    info: ChannelInfo,
    state: ChannelState,
    registry: Mutex<Option<Arc<ChannelRegistry>>>,
    replace_with_automatic: bool,
    sends: Arc<Sends>,
    successor_sends: Arc<Sends>,
}

impl VanishingExplicit {
    fn new(
        id: &str,
        replace_with_automatic: bool,
        sends: Arc<Sends>,
        successor_sends: Arc<Sends>,
    ) -> Self {
        Self {
            info: ChannelInfo {
                id: ChannelId::new(id),
                name: id.to_string(),
                channel_type: "explicit-mock".to_string(),
                status: ChannelStatus::Connected,
                capabilities: ChannelCapabilities::default(),
            },
            state: ChannelState::new(8),
            registry: Mutex::new(None),
            replace_with_automatic,
            sends,
            successor_sends,
        }
    }
}

#[async_trait]
impl Channel for VanishingExplicit {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }
    fn state(&self) -> &ChannelState {
        &self.state
    }
    fn outbound_delivery_policy(&self) -> OutboundDeliveryPolicy {
        OutboundDeliveryPolicy::ExplicitOnly
    }
    async fn start(&mut self) -> ChannelResult<()> {
        Ok(())
    }
    async fn stop(&mut self) -> ChannelResult<()> {
        self.sends.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn send(&self, _message: OutboundMessage) -> SendOperation {
        self.sends.attempts.fetch_add(1, Ordering::SeqCst);
        let registry = self.registry.lock().unwrap().clone();
        let id = self.info.id.clone();
        let replace_with_automatic = self.replace_with_automatic;
        let successor_sends = Arc::clone(&self.successor_sends);
        Box::pin(async move {
            if let Some(registry) = registry {
                let _ = registry.unregister(&id).await;
                if replace_with_automatic {
                    registry
                        .register(Box::new(MockChannel::new(
                            id.as_str(),
                            Arc::clone(&successor_sends),
                        )))
                        .await;
                }
            }
            Err(ChannelError::NotConnected("gone".into()))
        })
    }
}

/// Returns the retained `Sends` of the old channel and of the successor it
/// installs, so each instance's transport activity is independently observable.
async fn vanishing_fixture(
    replace: bool,
) -> (
    Arc<ChannelRegistry>,
    Arc<DeliveryStore>,
    Arc<Sends>,
    Arc<Sends>,
) {
    let store = Arc::new(DeliveryStore::open_in_memory(fast_cfg()).unwrap());
    let registry = Arc::new(ChannelRegistry::new().with_delivery_store(Arc::clone(&store)));
    let sends = Arc::new(Sends::default());
    let successor_sends = Arc::new(Sends::default());
    let channel = VanishingExplicit::new(
        "vanish",
        replace,
        Arc::clone(&sends),
        Arc::clone(&successor_sends),
    );
    *channel.registry.lock().unwrap() = Some(Arc::clone(&registry));
    registry.register(Box::new(channel)).await;
    (registry, store, sends, successor_sends)
}

#[tokio::test]
async fn explicit_only_channel_unregistered_mid_send_is_never_persisted() {
    let (registry, store, sends, _successor_sends) = vanishing_fixture(false).await;

    let err = tokio::time::timeout(
        Duration::from_secs(5),
        registry.send(
            &ChannelId::new("vanish"),
            OutboundMessage::text(PEER, "mid-send-secret"),
        ),
    )
    .await
    .expect("no deadlock")
    .expect_err("channel reports NotConnected");

    assert!(matches!(err, ChannelError::NotConnected(_)), "{err:?}");
    assert_eq!(sends.attempts.load(Ordering::SeqCst), 1);
    assert_nothing_stored(&store);
    assert_eq!(sends.stops.load(Ordering::SeqCst), 1);
    assert!(registry.get(&ChannelId::new("vanish")).await.is_none());
}

#[tokio::test]
async fn explicit_only_channel_replaced_mid_send_is_never_persisted() {
    let (registry, store, sends, successor_sends) = vanishing_fixture(true).await;

    let err = tokio::time::timeout(
        Duration::from_secs(5),
        registry.send(
            &ChannelId::new("vanish"),
            OutboundMessage::text(PEER, "mid-send-secret"),
        ),
    )
    .await
    .expect("no deadlock")
    .expect_err("channel reports NotConnected");

    assert!(matches!(err, ChannelError::NotConnected(_)), "{err:?}");
    assert_eq!(sends.attempts.load(Ordering::SeqCst), 1);
    assert_nothing_stored(&store);
    assert_eq!(sends.stops.load(Ordering::SeqCst), 1);
    assert_eq!(sends.attempts.load(Ordering::SeqCst), 1);
    let successor = registry
        .get(&ChannelId::new("vanish"))
        .await
        .expect("successor");
    assert_eq!(
        successor.read().await.outbound_delivery_policy(),
        OutboundDeliveryPolicy::Automatic
    );
    // The successor's read guard is released by the statement above, so the
    // real send below cannot deadlock against our own assertion.
    registry
        .send(
            &ChannelId::new("vanish"),
            OutboundMessage::text(PEER, "successor-visible"),
        )
        .await
        .expect("Automatic successor sends");
    assert_eq!(sends.attempts.load(Ordering::SeqCst), 1);
    assert_eq!(successor_sends.attempts.load(Ordering::SeqCst), 1);
    assert_eq!(
        successor_sends.texts.lock().unwrap().as_slice(),
        ["successor-visible".to_string()]
    );
    assert_nothing_stored(&store);
}

#[tokio::test]
async fn a_send_to_a_never_registered_channel_keeps_the_legacy_queueing() {
    // No ExplicitOnly witness exists for "ghost": legacy behaviour stays.
    let store = Arc::new(DeliveryStore::open_in_memory(fast_cfg()).unwrap());
    let registry = ChannelRegistry::new().with_delivery_store(Arc::clone(&store));
    let err = registry
        .send(&ChannelId::new("ghost"), OutboundMessage::text("c", "x"))
        .await
        .expect_err("not registered");
    assert!(matches!(err, ChannelError::NotConnected(_)));
    assert_eq!(
        store.len().unwrap(),
        1,
        "legacy: queued for a later registration"
    );
}

// ---- I4: retained payload scrub ---------------------------------------------------

/// Plain registry (no channel yet) over `store`.
fn registry_over(store: &Arc<DeliveryStore>) -> Arc<ChannelRegistry> {
    Arc::new(ChannelRegistry::new().with_delivery_store(Arc::clone(store)))
}

async fn register_sms(registry: &ChannelRegistry) -> Arc<FakeTransport> {
    let transport = FakeTransport::new(Mode::Accept);
    let channel = SmsChannel::new(SMS_ID, sms_config()).with_transport(transport.clone());
    registry.register(Box::new(channel)).await;
    transport
}

fn assert_letter_scrubbed(l: &alephcore::gateway::delivery_queue::DeadLetter) {
    assert!(
        !format!("{l:?}").contains(LEGACY_BODY),
        "dead letter still carries the plaintext body: {l:?}"
    );
    assert!(l.message.text.is_empty(), "body scrubbed");
    assert!(l.message.attachments.is_empty());
    assert!(l.message.metadata.is_empty());
    // History is not the replay predicate: policy retirement is sticky even
    // when the original Exhausted/Permanent reason was individually safe.
    assert!(
        l.retired_by_policy,
        "row must carry an independent retirement fence"
    );
    assert!(
        !l.redrivable(),
        "a bodyless retired row must never be redriven"
    );
}

#[tokio::test]
async fn registering_an_explicit_only_channel_promptly_scrubs_old_recognised_rows() {
    let store = Arc::new(DeliveryStore::open_in_memory(fast_cfg()).unwrap());
    let registry = registry_over(&store);
    registry
        .register(Box::new(MockChannel::new(
            SMS_ID,
            Arc::new(Sends::default()),
        )))
        .await;
    // Rows predate the registration of the channel that declares the policy.
    seed_legacy_row(&registry, &store, PEER, LEGACY_BODY).await;
    seed_legacy_row(&registry, &store, PEER, LEGACY_BODY).await;
    let id = store.claim_due(now_secs(), 10).unwrap()[0].id;
    store
        .record_dead_letter(
            id,
            3,
            &format!("failed carrying {LEGACY_BODY}"),
            alephcore::gateway::delivery_queue::DeadLetterReason::Exhausted,
        )
        .unwrap();

    register_sms(&registry).await;

    assert_eq!(store.len().unwrap(), 0, "live row retired at registration");
    let letters = store.recent_dead_letters(10).unwrap();
    assert_eq!(letters.len(), 2, "{letters:?}");
    for l in &letters {
        assert_letter_scrubbed(l);
    }
    assert_eq!(
        letters
            .iter()
            .filter(|l| l.reason.as_str() == "exhausted")
            .count(),
        1,
        "original historical reason remains, independent of row retirement"
    );
    assert_eq!(
        letters
            .iter()
            .filter(|l| l.reason.as_str() == "explicit_only_retired")
            .count(),
        1
    );
    // Redrive can neither resurrect the old Exhausted letter nor the retired one.
    let out = registry.redrive_dead_letters(None).expect("store attached");
    assert_eq!(out.moved, 0);
    assert_eq!(out.skipped_policy_retired, 2);
    assert_eq!(out.skipped_unsafe, 0, "no false historical ambiguity");
    assert_eq!(store.len().unwrap(), 0);
}

#[tokio::test]
async fn recovered_in_flight_sms_rows_are_scrubbed_and_stay_unknown_outcome() {
    let store = Arc::new(DeliveryStore::open_in_memory(fast_cfg()).unwrap());
    let registry = registry_over(&store);
    let transport = register_sms(&registry).await;
    seed_legacy_row(&registry, &store, PEER, LEGACY_BODY).await;
    let due = store.claim_due(now_secs(), 10).unwrap();
    store.mark_inflight(due[0].id, now_secs() - 5).unwrap();

    // Process "restart": reconcile copies in-flight rows verbatim, then the
    // drain's registry-aware pass must scrub them.
    spawn_drain(Arc::clone(&registry), Arc::clone(&store));
    for _ in 0..100 {
        let l = store.recent_dead_letters(10).unwrap();
        if l.len() == 1 && l[0].message.text.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }

    assert_eq!(transport.submits(), 0);
    assert_eq!(store.len().unwrap(), 0);
    let letters = store.recent_dead_letters(10).unwrap();
    assert_eq!(letters.len(), 1, "{letters:?}");
    assert_letter_scrubbed(&letters[0]);
    assert_eq!(
        letters[0].reason.as_str(),
        "unknown_outcome",
        "historical uncertainty is kept, not rewritten to 'never sent'"
    );
    assert!(!letters[0].reason.replay_safe());
    let out = registry.redrive_dead_letters(None).expect("store attached");
    assert_eq!(out.moved, 0);
    assert_eq!(out.skipped_unsafe, 1, "{out:?}");
}

#[tokio::test]
async fn retirement_is_sticky_after_unregister_and_across_a_restart() {
    let store = Arc::new(DeliveryStore::open_in_memory(fast_cfg()).unwrap());
    {
        let registry = registry_over(&store);
        registry
            .register(Box::new(MockChannel::new(
                SMS_ID,
                Arc::new(Sends::default()),
            )))
            .await;
        seed_legacy_row(&registry, &store, PEER, LEGACY_BODY).await;
        registry.unregister(&sms_id()).await;
        register_sms(&registry).await;
        registry.unregister(&sms_id()).await;
    }
    // A fresh registry over the same store: the SMS channel is not registered
    // (not yet restarted), but the store remembers it declared ExplicitOnly.
    let registry = registry_over(&store);
    spawn_drain(Arc::clone(&registry), Arc::clone(&store));
    for _ in 0..100 {
        if store.len().unwrap() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    assert_eq!(store.len().unwrap(), 0, "scrubbed, not rescheduled");
    let letters = store.recent_dead_letters(10).unwrap();
    assert_eq!(letters.len(), 1);
    assert_letter_scrubbed(&letters[0]);

    // A failed send to the now-unregistered channel is not persisted either.
    let _ = registry
        .send(&sms_id(), OutboundMessage::text(PEER, "new-secret"))
        .await
        .expect_err("not registered");
    assert_eq!(store.len().unwrap(), 0);
}

#[tokio::test]
async fn a_registered_automatic_channel_overrides_a_stale_explicit_marker() {
    let store = Arc::new(DeliveryStore::open_in_memory(fast_cfg()).unwrap());
    {
        let registry = registry_over(&store);
        register_sms(&registry).await;
    }
    let registry = registry_over(&store);
    // Same id now belongs to an ordinary channel.
    registry
        .register(Box::new(MockChannel::failing(
            SMS_ID,
            Arc::default(),
            Fail::NotConnected,
        )))
        .await;
    let _ = registry
        .send(&sms_id(), OutboundMessage::text("c", "ordinary"))
        .await
        .expect_err("down");
    assert_eq!(store.len().unwrap(), 1, "ordinary channel queues as before");
}
