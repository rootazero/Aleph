use super::*;
use aleph_client::CliConfig;
use aleph_protocol::panel_voice::{
    VOICE_TURN_CANCEL_METHOD, VOICE_TURN_CAPTURE_CANCEL_METHOD, VOICE_TURN_CAPTURE_FINISH_METHOD,
    VOICE_TURN_CAPTURE_METHOD, VOICE_TURN_CAPTURE_READY_METHOD, VOICE_TURN_SYNTHESIZE_METHOD,
    VOICE_TURN_TRANSCRIBE_METHOD,
};
use futures_util::{poll, SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::{accept_async, tungstenite::Message, WebSocketStream};

type Peer = WebSocketStream<TcpStream>;

// Canonical mono 16 kHz PCM16 WAV with one silent sample.
const INLINE_WAV: &str = "UklGRiYAAABXQVZFZm10IBAAAAABAAEAgD4AAAB9AAACABAAZGF0YQIAAAAAAA==";

fn host() -> Handle {
    Handle::start(
        HostChannel::default(),
        Target::Unavailable("test transport is installed explicitly"),
        true,
        Arc::default(),
        || {},
    )
    .unwrap()
}

// Use real client dispatch, wire ordering, and responses on Windows too, where
// production desktop IPC is disabled. Only the test carrier is a WebSocket.
async fn install(shared: &Shared) -> (Arc<AlephClient>, Peer, Value) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let config = CliConfig::default();
    let (client, peer) = tokio::join!(AlephClient::connect(&url, &config), async {
        let mut peer = accept_async(listener.accept().await.unwrap().0)
            .await
            .unwrap();
        let hello = wire(&mut peer, "connect").await;
        reply(&mut peer, hello, json!({"role":"operator"})).await;
        peer
    });
    let client = Arc::new(client.unwrap().0);
    shared.link().install_client(client.clone());
    let binding = shared.connection()["voice_binding"].clone();
    assert!(binding.is_string());
    (client, peer, binding)
}

async fn wire(peer: &mut Peer, method: &str) -> Value {
    wire_request(peer, method).await["id"].clone()
}

async fn wire_request(peer: &mut Peer, method: &str) -> Value {
    let message = peer.next().await.unwrap().unwrap();
    let request: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
    assert_eq!(request["method"], method);
    assert!(request.get("voice_binding").is_none());
    request
}

async fn reply(peer: &mut Peer, id: Value, result: Value) {
    let json = json!({"jsonrpc":"2.0","id":id,"result":result});
    peer.send(Message::Text(json.to_string().into()))
        .await
        .unwrap();
}

fn bound(method: &str, params: Value, binding: &Value) -> Value {
    json!({"method":method,"params":params,"voice_binding":binding})
}

#[test]
fn panel_host_queued_voice_rpc_cannot_dispatch_to_the_replacement_client() {
    let host = host();
    host.runtime.as_ref().unwrap().block_on(async {
        timeout(Duration::from_secs(5), async {
            let shared = &host.shared;
            for method in [VOICE_TURN_SYNTHESIZE_METHOD, "chat.send"] {
                let (old_client, mut old_peer, old_binding) = install(shared).await;
                let mut blocker = std::pin::pin!(shared.rpc(json!({"method": "sessions.list"})));
                assert!(poll!(&mut blocker).is_pending());
                let held = wire(&mut old_peer, "sessions.list").await;
                let mut old = std::pin::pin!(shared.rpc(bound(method, Value::Null, &old_binding)));
                assert!(poll!(&mut old).is_pending());
                // No second waiting voice task is admitted behind ordinary RPCs.
                let excess = shared.admit_rpc(&bound(method, Value::Null, &old_binding));
                assert!(matches!(excess, Err(reply) if reply["error"]["message"] == "the voice RPC lane is busy"));
                shared.lost(&old_client);
                assert!(shared.connection().get("voice_binding").is_none());
                let (_client, mut replacement, new_binding) = install(shared).await;
                assert_ne!(old_binding, new_binding);
                reply(&mut old_peer, held, json!(true)).await;
                assert_eq!(blocker.await, json!({"result":true}));
                assert_eq!(old.await["error"]["code"], PANEL_HOST_UNAVAILABLE);

                let mut chat = std::pin::pin!(shared.rpc(json!({"method":"chat.send"})));
                assert!(poll!(&mut chat).is_pending());
                let ordinary = wire(&mut replacement, "chat.send").await;
                reply(&mut replacement, ordinary, json!(true)).await;
                assert_eq!(chat.await, json!({"result":true}));
            }
            let no_binding = shared
                .rpc(json!({"method":VOICE_TURN_SYNTHESIZE_METHOD}))
                .await;
            assert_eq!(no_binding["error"]["code"], PANEL_HOST_UNAVAILABLE);

        })
        .await
        .unwrap();
    });
}

#[test]
fn panel_host_capture_and_single_voice_cancel_dispatch_while_generic_turn_is_held() {
    let host = host();
    host.runtime.as_ref().unwrap().block_on(async {
        timeout(Duration::from_secs(5), async {
            let shared = &host.shared;
            let (_client, mut peer, binding) = install(shared).await;
            let mut synth = std::pin::pin!(shared.rpc(bound(
                VOICE_TURN_SYNTHESIZE_METHOD,
                Value::Null,
                &binding,
            )));
            assert!(poll!(&mut synth).is_pending());
            let synthesis = wire(&mut peer, VOICE_TURN_SYNTHESIZE_METHOD).await;

            let mut cancel =
                std::pin::pin!(shared.rpc(bound(VOICE_TURN_CANCEL_METHOD, Value::Null, &binding,)));
            assert!(poll!(&mut cancel).is_pending());
            let cancel_id = wire(&mut peer, VOICE_TURN_CANCEL_METHOD).await;
            let duplicate = shared
                .rpc(bound(VOICE_TURN_CANCEL_METHOD, Value::Null, &binding))
                .await;
            assert_eq!(duplicate["error"]["code"], PANEL_HOST_UNAVAILABLE);
            reply(&mut peer, cancel_id, json!(true)).await;
            assert_eq!(cancel.await, json!({"result":true}));

            let capture_id = Uuid::new_v4();
            let params = json!({"capture_id": capture_id});
            let mut capture = std::pin::pin!(shared.rpc(bound(
                VOICE_TURN_CAPTURE_METHOD,
                params.clone(),
                &binding,
            )));
            assert!(poll!(&mut capture).is_pending());
            let started = wire(&mut peer, VOICE_TURN_CAPTURE_METHOD).await;
            for (method, params) in [
                (VOICE_TURN_CAPTURE_READY_METHOD, params.clone()),
                (VOICE_TURN_CAPTURE_FINISH_METHOD, params.clone()),
                (
                    VOICE_TURN_CAPTURE_CANCEL_METHOD,
                    json!({"capture_id": Uuid::new_v4()}),
                ),
            ] {
                let mut control = std::pin::pin!(shared.rpc(bound(method, params, &binding)));
                assert!(poll!(&mut control).is_pending());
                let id = wire(&mut peer, method).await;
                reply(&mut peer, id, json!(true)).await;
                assert_eq!(control.await, json!({"result":true}));
            }
            host.set_visible(false);
            host.set_visible(false);
            let message = peer.next().await.unwrap().unwrap();
            let hide_cancel: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
            assert_eq!(hide_cancel["method"], VOICE_TURN_CAPTURE_CANCEL_METHOD);
            assert_eq!(hide_cancel["params"], json!({ "capture_id": capture_id }));
            reply(&mut peer, hide_cancel["id"].clone(), json!({})).await;
            reply(&mut peer, started, json!(true)).await;
            assert_eq!(capture.await["error"]["code"], PANEL_HOST_UNAVAILABLE);
            let hidden = shared
                .rpc(bound(VOICE_TURN_CAPTURE_METHOD, params, &binding))
                .await;
            assert_eq!(hidden["error"]["code"], PANEL_HOST_UNAVAILABLE);
            let mut next_cancel =
                std::pin::pin!(shared.rpc(bound(VOICE_TURN_CANCEL_METHOD, Value::Null, &binding)));
            assert!(poll!(&mut next_cancel).is_pending());
            let id = wire(&mut peer, VOICE_TURN_CANCEL_METHOD).await;
            reply(&mut peer, id, json!(true)).await;
            assert_eq!(next_cancel.await, json!({"result":true}));
            reply(&mut peer, synthesis, json!(true)).await;
            assert_eq!(synth.await, json!({"result":true}));
        })
        .await
        .unwrap();
    });
}

async fn seal_capture(shared: &Shared, peer: &mut Peer, binding: &Value, id: Uuid) {
    let mut capture = std::pin::pin!(shared.rpc(bound(
        VOICE_TURN_CAPTURE_METHOD,
        json!({"capture_id": id}),
        binding,
    )));
    assert!(poll!(&mut capture).is_pending());
    let begin = wire(peer, VOICE_TURN_CAPTURE_METHOD).await;
    reply(peer, begin, json!({"limit_reached": false})).await;
    assert_eq!(capture.await, json!({"result": {"limit_reached": false}}));
    assert!(
        shared.link().capture.is_some(),
        "a sealed clip is still owned by the host"
    );
}

#[test]
fn panel_host_hide_cancels_sealed_capture_on_reserved_third_control_and_blocks_queued_transcribe() {
    let host = host();
    host.runtime.as_ref().unwrap().block_on(async {
        timeout(Duration::from_secs(5), async {
            let shared = &host.shared;
            let (_client, mut peer, binding) = install(shared).await;
            let capture_id = Uuid::new_v4();
            let params = json!({"capture_id": capture_id});
            seal_capture(shared, &mut peer, &binding, capture_id).await;

            let escaped = json!({"capture_id":capture_id, "extra":"\n".repeat(2024)});
            assert!(matches!(shared.admit_rpc(&bound(VOICE_TURN_CAPTURE_READY_METHOD, escaped, &binding)),
                Err(reply) if reply["error"]["message"] == "voice RPC exceeds its wire budget"));
            let mut ready = std::pin::pin!(shared.rpc(bound(VOICE_TURN_CAPTURE_READY_METHOD, params.clone(), &binding)));
            let mut finish = std::pin::pin!(shared.rpc(bound(VOICE_TURN_CAPTURE_FINISH_METHOD, params.clone(), &binding)));
            assert!(poll!(&mut ready).is_pending());
            assert!(poll!(&mut finish).is_pending());
            let ready_id = wire(&mut peer, VOICE_TURN_CAPTURE_READY_METHOD).await;
            let finish_id = wire(&mut peer, VOICE_TURN_CAPTURE_FINISH_METHOD).await;
            assert!(matches!(shared.admit_rpc(&bound(VOICE_TURN_CAPTURE_CANCEL_METHOD, params.clone(), &binding)),
                Err(reply) if reply["error"]["message"] == "the voice RPC lane is busy"));

            let turn = shared.turn.lock().await;
            let mut consume = std::pin::pin!(shared.rpc(bound(VOICE_TURN_TRANSCRIBE_METHOD,
                json!({"capture_id": capture_id, "turn_id": Uuid::new_v4()}), &binding)));
            assert!(poll!(&mut consume).is_pending());
            host.set_visible(false);
            host.set_visible(true);
            let message = peer.next().await.unwrap().unwrap();
            let cleanup: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
            assert_eq!(cleanup["method"], VOICE_TURN_CAPTURE_CANCEL_METHOD);
            assert_eq!(cleanup["params"], params);
            assert!(shared.link().capture.is_some(), "sending cleanup is not proof of release");
            drop(turn);
            assert_eq!(consume.await["error"]["message"], "the voice surface is no longer visible in this epoch");
            reply(&mut peer, cleanup["id"].clone(), json!({})).await;
            while shared.link().capture.is_some() { tokio::task::yield_now().await; }
            reply(&mut peer, finish_id, json!({})).await;
            reply(&mut peer, ready_id, json!({"ready":true})).await;
            assert_eq!(finish.await, json!({"result":{}}));
            assert_eq!(ready.await, json!({"result":{"ready":true}}));

            // Successful consume releases the exact owner; hide must not send
            // its capture cancellation ahead of the next ordinary wire call.
            let next = Uuid::new_v4();
            seal_capture(shared, &mut peer, &binding, next).await;
            let mut transcribe = std::pin::pin!(shared.rpc(bound(VOICE_TURN_TRANSCRIBE_METHOD,
                json!({"capture_id": next, "turn_id": Uuid::new_v4()}), &binding)));
            assert!(poll!(&mut transcribe).is_pending());
            let sent = wire(&mut peer, VOICE_TURN_TRANSCRIBE_METHOD).await;
            reply(&mut peer, sent, json!({"text":"fixture"})).await;
            assert_eq!(transcribe.await, json!({"result":{"text":"fixture"}}));
            assert!(shared.link().capture.is_none());
            host.set_visible(false);
            let mut ordinary = std::pin::pin!(shared.rpc(json!({"method":"health"})));
            assert!(poll!(&mut ordinary).is_pending());
            let sent = wire(&mut peer, "health").await;
            reply(&mut peer, sent, json!(true)).await;
            assert_eq!(ordinary.await, json!({"result":true}));
        }).await.unwrap();
    });
}

#[test]
fn panel_host_failed_sealed_cleanup_retires_only_its_client_without_reconnect() {
    let host = host();
    host.runtime.as_ref().unwrap().block_on(async {
        timeout(Duration::from_secs(5), async {
            let shared = &host.shared;
            let (client, mut peer, binding) = install(shared).await;
            let original = Uuid::new_v4();
            seal_capture(shared, &mut peer, &binding, original).await;
            host.set_visible(false);
            let failed = wire(&mut peer, VOICE_TURN_CAPTURE_CANCEL_METHOD).await;
            let response = json!({"jsonrpc":"2.0","id":failed,"error":{"code":-32603,"message":"cleanup failed"}});
            peer.send(Message::Text(response.to_string().into())).await.unwrap();
            // Local retirement causes actual peer EOF/reset, not only a host status flip.
            assert!(!matches!(peer.next().await, Some(Ok(_))));
            assert!(!client.is_connected());
            while shared.connection()["state"] != "disconnected" { tokio::task::yield_now().await; }
            assert!(shared.connection().get("voice_binding").is_none());
            assert!(shared.link().client.is_none());

            host.set_visible(true);
            let (original, mut old_peer, old_binding) = install(shared).await;
            seal_capture(shared, &mut old_peer, &old_binding, Uuid::new_v4()).await;
            host.set_visible(false);
            let late = wire(&mut old_peer, VOICE_TURN_CAPTURE_CANCEL_METHOD).await;
            // Simulate a newer authenticated connection by the time the old
            // cleanup returns. Its failure can only retire its captured client.
            let (replacement, mut new_peer, new_binding) = install(shared).await;
            let response = json!({"jsonrpc":"2.0","id":late,"error":{"code":-32603,"message":"cleanup failed"}});
            old_peer.send(Message::Text(response.to_string().into())).await.unwrap();
            assert!(!matches!(old_peer.next().await, Some(Ok(_))));
            assert!(!original.is_connected());
            assert!(replacement.is_connected());
            assert_eq!(shared.connection()["voice_binding"], new_binding);
            let mut ordinary = std::pin::pin!(shared.rpc(json!({"method":"health"})));
            assert!(poll!(&mut ordinary).is_pending());
            let sent = wire(&mut new_peer, "health").await;
            reply(&mut new_peer, sent, json!(true)).await;
            assert_eq!(ordinary.await, json!({"result":true}));
        }).await.unwrap();
    });
}

#[test]
fn panel_host_late_chat_receipt_aborts_exact_run_while_new_transcribe_is_on_wire() {
    let host = host();
    host.runtime.as_ref().unwrap().block_on(async {
        timeout(Duration::from_secs(5), async {
            let shared = &host.shared;
            let (_, mut peer, binding) = install(shared).await;
            let mut send = std::pin::pin!(shared.rpc(bound(
                "chat.send",
                json!({"message":"old turn"}),
                &binding
            )));
            assert!(poll!(&mut send).is_pending());
            let sent = wire(&mut peer, "chat.send").await;
            let run_id = Uuid::new_v4();
            reply(
                &mut peer,
                sent,
                json!({"run_id":run_id,"session_key":"fixture"}),
            )
            .await;
            let receipt = send.await;
            assert_eq!(receipt["result"]["run_id"], json!(run_id));

            // The send completed in the host; its receipt is only now acted
            // on by the surface, after a new Voice turn acquired the serial lane.
            let mut transcribe = std::pin::pin!(shared.rpc(bound(
                VOICE_TURN_TRANSCRIBE_METHOD,
                json!({"turn_id":Uuid::new_v4(),"audio_base64":INLINE_WAV}),
                &binding
            )));
            assert!(poll!(&mut transcribe).is_pending());
            let transcription = wire(&mut peer, VOICE_TURN_TRANSCRIBE_METHOD).await;
            let params = json!({"run_id":receipt["result"]["run_id"],"run_only":true});
            for invalid in [
                json!({"run_id":run_id}),
                json!({"run_id":run_id,"run_only":false}),
                json!({"run_id":run_id,"run_only":true,"session_key":"fixture"}),
            ] {
                let widened = shared.rpc(bound("chat.abort", invalid, &binding)).await;
                assert_eq!(widened["error"]["code"], -32602);
            }
            let mut abort =
                std::pin::pin!(shared.rpc(bound("chat.abort", params.clone(), &binding)));
            assert!(poll!(&mut abort).is_pending());
            let abort_wire = wire_request(&mut peer, "chat.abort").await;
            assert_eq!(abort_wire["params"], params);
            assert!(
                matches!(shared.admit_rpc(&bound("chat.abort", params, &binding)),
                Err(reply) if reply["error"]["message"] == "the voice RPC lane is busy")
            );
            reply(&mut peer, abort_wire["id"].clone(), json!({"aborted":true})).await;
            assert_eq!(abort.await, json!({"result":{"aborted":true}}));
            assert!(poll!(&mut transcribe).is_pending());

            let ordinary_params = json!({"run_id":run_id,"session_key":"fixture"});
            let mut ordinary =
                std::pin::pin!(shared.rpc(json!({"method":"chat.abort","params":ordinary_params})));
            assert!(poll!(&mut ordinary).is_pending());
            reply(&mut peer, transcription, json!({"text":"new turn"})).await;
            assert_eq!(transcribe.await, json!({"result":{"text":"new turn"}}));
            assert!(poll!(&mut ordinary).is_pending());
            let ordinary_wire = wire_request(&mut peer, "chat.abort").await;
            assert_eq!(ordinary_wire["params"], ordinary_params);
            reply(
                &mut peer,
                ordinary_wire["id"].clone(),
                json!({"aborted":false}),
            )
            .await;
            assert_eq!(ordinary.await, json!({"result":{"aborted":false}}));
        })
        .await
        .unwrap();
    });
}

#[test]
fn panel_host_rpc_handler_sheds_ordinary_backlog_before_spawn() {
    use tokio::sync::oneshot;

    let host = host();
    host.runtime.as_ref().unwrap().block_on(async {
        timeout(Duration::from_secs(5), async {
            let shared = &host.shared;
            let (_, mut peer, binding) = install(shared).await;
            let turn = shared.turn.lock().await;
            let submit = |data| {
                let (tx, rx) = oneshot::channel();
                // Exact aleph.rpc callback path after HostCall relinquishes its value.
                shared.clone().start_rpc(data, move |reply| {
                    let _ = tx.send(reply);
                });
                rx
            };
            let mut queued: Vec<_> = (0..ORDINARY_RPC_SLOTS)
                .map(|_| submit(json!({"method":"health"})))
                .collect();
            assert!(queued
                .iter_mut()
                .all(|rx| rx.try_recv() == Err(oneshot::error::TryRecvError::Empty)));
            let mut excess = submit(json!({"method":"health"}));
            let reply_value = excess
                .try_recv()
                .expect("capacity refusal must be synchronous");
            assert_eq!(
                reply_value["error"]["message"],
                "the ordinary RPC queue is full"
            );
            let cancel = submit(bound(
                VOICE_TURN_CANCEL_METHOD,
                json!({"turn_id":Uuid::new_v4()}),
                &binding,
            ));
            let id = wire(&mut peer, VOICE_TURN_CANCEL_METHOD).await;
            reply(&mut peer, id, Value::Null).await;
            assert_eq!(cancel.await.unwrap(), json!({"result":null}));
            drop(turn);
            for _ in 0..ORDINARY_RPC_SLOTS {
                let id = wire(&mut peer, "health").await;
                reply(&mut peer, id, json!(true)).await;
            }
            for done in queued {
                assert_eq!(done.await.unwrap(), json!({"result":true}));
            }
            let oversized = "x".repeat(aleph_protocol::framing::DEFAULT_MAX_PAYLOAD_BYTES as usize);
            let mut refused = submit(json!({"method":"health","params":{"large":oversized}}));
            let reply_value = refused
                .try_recv()
                .expect("oversize refusal must be synchronous");
            assert_eq!(
                reply_value["error"]["message"],
                "ordinary RPC exceeds its wire budget"
            );
        })
        .await
        .unwrap();
    });
}

#[test]
fn panel_host_inline_transcribe_queued_before_hide_show_never_reaches_wire() {
    let host = host();
    host.runtime.as_ref().unwrap().block_on(async {
        timeout(Duration::from_secs(5), async {
            let shared = &host.shared;
            let (_, mut peer, binding) = install(shared).await;
            let turn = shared.turn.lock().await;
            let mut queued = std::pin::pin!(shared.rpc(bound(VOICE_TURN_TRANSCRIBE_METHOD,
                json!({"turn_id":Uuid::new_v4(),"audio_base64":INLINE_WAV}), &binding)));
            assert!(poll!(&mut queued).is_pending());
            assert!(shared.link().capture.is_none());
            host.set_visible(false);
            host.set_visible(true);
            drop(turn);
            let rejected = tokio::select! {
                reply = &mut queued => reply,
                frame = peer.next() => panic!("stale inline transcribe reached the Gateway wire: {frame:?}"),
            };
            assert_eq!(rejected["error"]["message"], "the voice surface is no longer visible in this epoch");
            let mut barrier = std::pin::pin!(shared.rpc(json!({"method":"health"})));
            assert!(poll!(&mut barrier).is_pending());
            let id = wire(&mut peer, "health").await;
            reply(&mut peer, id, json!(true)).await;
            assert_eq!(barrier.await, json!({"result":true}));
        }).await.unwrap();
    });
}

#[test]
fn panel_host_rpc_forwards_structured_error_data_and_preserves_generic_errors() {
    let host = host();
    host.runtime.as_ref().unwrap().block_on(async {
        timeout(Duration::from_secs(5), async {
            let shared = &host.shared;
            let (_client, mut peer, _binding) = install(shared).await;
            let data = json!({
                "reconciliation_required": true,
                "address": "agent/default/notes/example",
                "revision": 7
            });
            for error in [
                json!({"code": -32000, "message": "reconcile", "data": data}),
                json!({"code": -32601, "message": "method unavailable"}),
            ] {
                let mut call = std::pin::pin!(shared.rpc(json!({
                    "method": "memory.graph.update", "params": {}
                })));
                assert!(poll!(&mut call).is_pending());
                let id = wire(&mut peer, "memory.graph.update").await;
                peer.send(Message::Text(
                    json!({
                        "jsonrpc": "2.0", "id": id, "error": error
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
                assert_eq!(call.await, json!({"error": error}));
            }
        })
        .await
        .unwrap();
    });
}
