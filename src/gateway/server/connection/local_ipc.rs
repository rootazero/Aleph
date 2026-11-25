//! Opt-in, same-account Desktop Gateway ingress. No legacy network privileges apply.

use std::{
    io,
    net::{IpAddr, Ipv4Addr},
    path::Path,
    time::Duration,
};

use aleph_client::local_ipc::{
    parse_payload, parse_request_payload, parse_voice_response_payload, read_frame, write_frame,
    HELLO_TIMEOUT, MAX_FRAME_BYTES, MAX_INFLIGHT,
};
use aleph_protocol::panel_voice::VOICE_TURN_CANCEL_METHOD;
use aleph_protocol::system_hello::{Feature, HelloLimits};
use futures_util::{stream::FuturesUnordered, StreamExt};
use serde_json::Value;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{oneshot, OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
    time::{sleep_until, timeout, Instant},
};
use tokio_util::sync::CancellationToken;

use super::{
    cleanup,
    dispatch::{active_desktop_owner, DispatchPeer},
    event_admission::{next_received, V1Event, V1EventFeed},
    reply_writer::Writes,
    rpc, ConnectionContext,
};
use crate::{
    cluster::PendingInvokes,
    gateway::{
        handlers::system_hello,
        lane::ChannelClass,
        protocol::{
            JsonRpcRequest, JsonRpcResponse, AUTH_REQUIRED, INTERNAL_ERROR, METHOD_NOT_FOUND,
        },
        security::store::OWNER_USER_ID,
        server::{ConnectionState, GatewaySharedState},
    },
    sync_primitives::Arc,
};

#[cfg(unix)]
use tokio::net::UnixListener;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(in crate::gateway::server) use windows::{bind, serve};

const MAX_CONNECTIONS: usize = 8;
const WIRE_BUDGET: usize = 16 * 1024 * 1024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const NO_NETWORK_ORIGIN: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

#[cfg(unix)]
pub(in crate::gateway::server) fn bind(path: &Path) -> io::Result<UnixListener> {
    let path = aleph_client::local_ipc::unix::private_endpoint(path)?;
    // Bind never replaces an occupied entry. The caller owns the directory
    // and supplies a fresh path; we never mutate its entries after bind.
    UnixListener::bind(path)
}

#[cfg(unix)]
pub(in crate::gateway::server) async fn serve(
    listener: UnixListener,
    state: Arc<GatewaySharedState>,
    stop: CancellationToken,
) -> io::Result<()> {
    let mut tasks = JoinSet::new();
    let result = loop {
        tokio::select! {
            biased;
            _ = stop.cancelled() => break Ok(()),
            Some(completed) = tasks.join_next(), if !tasks.is_empty() => {
                if let Err(error) = completed {
                    tracing::warn!(%error, "Desktop IPC connection task failed");
                }
            }
            accepted = listener.accept(), if tasks.len() < MAX_CONNECTIONS => {
                let stream = match accepted {
                    Ok((stream, _)) => stream,
                    Err(error) => break Err(error),
                };
                if aleph_client::local_ipc::unix::require_same_user(&stream).is_err() {
                    continue;
                }
                let state = state.clone();
                let stop = stop.clone();
                tasks.spawn(async move {
                    let (read, write) = stream.into_split();
                    let mut cleanup = None;
                    if let Err(error) = serve_peer(read, write, state, stop, &mut cleanup).await {
                        tracing::debug!(kind = ?error.kind(), "Desktop IPC peer closed");
                    }
                    if let Some(cleanup) = cleanup {
                        cleanup.finish().await;
                    }
                });
            }
        }
    };
    stop.cancel();
    drop(listener);
    while let Some(completed) = tasks.join_next().await {
        if let Err(error) = completed {
            tracing::warn!(%error, "Desktop IPC connection teardown failed");
        }
    }
    result
}

async fn serve_peer(
    mut read: impl AsyncRead + Unpin,
    mut write: impl AsyncWrite + Unpin,
    state: Arc<GatewaySharedState>,
    stop: CancellationToken,
    cleanup: &mut Option<PeerCleanup>,
) -> io::Result<()> {
    let Some(store) = state
        .security_store
        .clone()
        .filter(|store| active_desktop_owner(store).is_some())
    else {
        return Ok(());
    };
    let id = format!("ipc-{}", uuid::Uuid::new_v4());
    let hello = tokio::select! {
        biased;
        _ = stop.cancelled() => return Ok(()),
        result = timeout(HELLO_TIMEOUT, read_request(&mut read)) => result.map_err(io::Error::other)??,
    };
    let Some((_, request)) = hello else {
        return Ok(());
    };
    let (hello, granted) = if request.method == "system.hello" {
        system_hello::negotiate(
            request,
            id.clone(),
            HelloLimits {
                max_message_bytes: u64::from(MAX_FRAME_BYTES),
                max_inflight_requests: MAX_INFLIGHT,
            },
            system_hello::subject_for(&store, OWNER_USER_ID),
            super::V1_FEATURES,
        )
    } else {
        (
            JsonRpcResponse::error(request.id, AUTH_REQUIRED, "system.hello must be first"),
            Vec::new(),
        )
    };
    if let Some(error) = &hello.error {
        let response = serialize(&hello)?;
        let response = if response_fits(&response, None) {
            response
        } else {
            serialize(&JsonRpcResponse::error(
                hello.id.clone(),
                error.code,
                "system.hello rejected",
            ))?
        };
        return send(&mut write, &response).await;
    }
    let Some(role) = active_desktop_owner(&store) else {
        return Ok(());
    };
    {
        let mut connections = state.connections.write().await;
        if connections.len() >= state.max_connections || stop.is_cancelled() {
            return Ok(());
        }
        let mut row = ConnectionState::new(NO_NETWORK_ORIGIN, false);
        row.first_message = false;
        row.caller_user = Some(OWNER_USER_ID.to_owned());
        row.caller_role = role.to_owned();
        connections.insert(id.clone(), row);
    }
    let ctx =
        ConnectionContext::from_shared(&state, ChannelClass::Desktop, NO_NETWORK_ORIGIN, false);
    let cancellation = stop.child_token();
    let _cancel_on_close = cancellation.clone().drop_guard();
    let peer = DispatchPeer::desktop(state.connections.clone(), id.clone(), store, cancellation);
    let executor = rpc::RpcExecutor {
        middleware: ctx.middleware_chain.clone(),
        lanes: ctx.lane_manager.clone(),
        idempotency: ctx.idempotency_guard.clone(),
        class: ctx.channel_class,
    };
    // Subscribed before the hello reply is written, so no frame published
    // after the client learns it may expect events is missed.
    let mut events = granted
        .contains(&Feature::LiveEvents)
        .then(|| V1EventFeed::subscribe(&ctx, id.clone(), None, MAX_FRAME_BYTES as usize));
    let result = tokio::select! {
        biased;
        _ = stop.cancelled() => Ok(()),
        result = async {
            send(&mut write, &serialize(&hello)?).await?;
            // Ordinary jobs own their slot through write acknowledgment. One
            // live event and one control reply have independent slots.
            let (writes, mut outgoing) = Writes::<IpcWrite>::queue(MAX_INFLIGHT as usize + 2);
            let writer = async {
                while let Some(item) = outgoing.recv().await {
                    send(&mut write, &item.frame.text).await?;
                    let _ = item.sent.send(());
                }
                Ok(())
            };
            tokio::select! {
                biased;
                result = writer => result,
                result = serve_requests(&mut read, &writes, &executor, &peer, &ctx, &mut events) => result,
            }
        } => result,
    };
    *cleanup = Some(PeerCleanup { id, ctx, peer });
    result
}

struct PeerCleanup {
    id: String,
    ctx: ConnectionContext,
    peer: DispatchPeer,
}

impl PeerCleanup {
    async fn finish(self) {
        self.peer.drain_desktop_voice().await;
        cleanup::run_cleanup(&self.id, &self.ctx, &Arc::new(PendingInvokes::new())).await;
    }
}

struct IpcWrite {
    text: String,
    _input_permit: Option<OwnedSemaphorePermit>,
}

impl IpcWrite {
    fn without_input_permit(text: String) -> Self {
        Self {
            text,
            _input_permit: None,
        }
    }
}

async fn read_request(
    reader: &mut (impl AsyncRead + Unpin),
) -> io::Result<Option<(String, JsonRpcRequest)>> {
    read_frame(reader)
        .await?
        .map(|text| parse_request(&text).map(|request| (text, request)))
        .transpose()
}

fn parse_request(text: &str) -> io::Result<JsonRpcRequest> {
    let request: JsonRpcRequest =
        serde_json::from_value(parse_request_payload(text)?).map_err(io::Error::other)?;
    if request.jsonrpc != "2.0"
        || !request
            .id
            .as_ref()
            .is_some_and(|id| id.is_string() || id.is_i64() || id.is_u64())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid IPC RPC envelope",
        ));
    }
    Ok(request)
}

async fn serve_requests(
    read: &mut (impl AsyncRead + Unpin),
    writes: &Writes<IpcWrite>,
    executor: &rpc::RpcExecutor,
    peer: &DispatchPeer,
    ctx: &ConnectionContext,
    events: &mut Option<V1EventFeed>,
) -> io::Result<()> {
    let mut jobs = FuturesUnordered::new();
    let budget = std::sync::Arc::new(Semaphore::new(WIRE_BUDGET));
    let mut active: Vec<Value> = Vec::new();
    let mut pending_replies: Vec<(Value, oneshot::Receiver<()>)> = Vec::new();
    let mut pending_control: Option<(Value, oneshot::Receiver<()>)> = None;
    let mut pending_event: Option<oneshot::Receiver<()>> = None;
    let mut lost_deadline = None;
    loop {
        // Stage one hard-capped frame without waiting for a job's byte lease.
        // Preserve partial reads across writes, including the terminal warning.
        let frame = read_frame(read);
        tokio::pin!(frame);
        let next = loop {
            tokio::select! {
                biased;
                ack = async { pending_event.as_mut().expect("pending event").await }, if pending_event.is_some() => {
                    ack.map_err(io::Error::other)?;
                    pending_event = None;
                    if lost_deadline.is_some() {
                        return Ok(());
                    }
                }
                _ = async { sleep_until(lost_deadline.expect("terminal event deadline")).await }, if lost_deadline.is_some() => {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "IPC event loss warning timed out"));
                }
                completed = jobs.next(), if !jobs.is_empty() && lost_deadline.is_none() => {
                    if let Some((id, response, voice_synthesis, permit)) = completed {
                        let id: Value = id;
                        let voice_synthesis: bool = voice_synthesis;
                        let response: String = response?;
                        let text = ipc_response(response, &id, voice_synthesis)?;
                        let ack = enqueue(writes, IpcWrite { text, _input_permit: Some(permit) })?;
                        pending_replies.push((id, ack));
                    }
                }
                // Live events: admitted in the body, not the branch, so a
                // completion or read winning the race never drops a frame.
                received = next_received(events), if pending_event.is_none() => {
                    let feed = events.as_mut().expect("only a feed yields frames");
                    let frame = match feed.admit(ctx, received).await {
                        None => continue,
                        Some(V1Event::Send(frame)) => frame,
                        Some(V1Event::Lost(warning)) => {
                            lost_deadline = Some(Instant::now() + WRITE_TIMEOUT);
                            warning
                        }
                        Some(V1Event::Close(_)) => return Ok(()),
                    };
                    pending_event = Some(enqueue(writes, IpcWrite::without_input_permit(frame))?);
                }
                request = &mut frame => break request?,
            }
        };
        let Some(text) = next else {
            return Ok(());
        };
        // Keep observing the transport, but don't let invalid RPC payloads
        // interrupt the terminal warning before its write acknowledgment.
        if lost_deadline.is_some() {
            continue;
        }
        let request = parse_request(&text)?;
        let id = request.id.clone().expect("validated request id");
        pending_replies.retain_mut(|(id, ack)| {
            if ack.try_recv().is_ok() {
                active.retain(|entry| entry != id);
                return false;
            }
            true
        });
        if pending_control
            .as_mut()
            .is_some_and(|(_, ack)| ack.try_recv().is_ok())
        {
            pending_control = None;
        }
        if active.contains(&id)
            || pending_control
                .as_ref()
                .is_some_and(|(pending, _)| pending == &id)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "duplicate IPC request id",
            ));
        }
        if request.method == VOICE_TURN_CANCEL_METHOD {
            // Reject control pipelining before it can execute or allocate a reply.
            if pending_control.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "IPC control request limit exceeded",
                ));
            }
            let response = execute(executor, peer, &text, &request).await?;
            let text = ipc_response(response, &id, false)?;
            let ack = enqueue(writes, IpcWrite::without_input_permit(text))?;
            pending_control = Some((id, ack));
            continue;
        }
        if active.len() >= MAX_INFLIGHT as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "IPC request limit exceeded",
            ));
        }
        // A staged ordinary frame must acquire its complete byte budget now
        // or close the peer, never wait while it owns the only reader.
        let permit = budget
            .clone()
            .try_acquire_many_owned(text.len() as u32)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "IPC request byte budget exceeded",
                )
            })?;
        active.push(id.clone());
        jobs.push(async move {
            let result = execute(executor, peer, &text, &request).await;
            let voice_synthesis =
                request.method == aleph_protocol::panel_voice::VOICE_TURN_SYNTHESIZE_METHOD;
            (id, result, voice_synthesis, permit)
        });
    }
}

fn enqueue(writes: &Writes<IpcWrite>, frame: IpcWrite) -> io::Result<oneshot::Receiver<()>> {
    writes
        .enqueue(frame)
        .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "IPC response queue exceeded"))
}

async fn execute(
    executor: &rpc::RpcExecutor,
    peer: &DispatchPeer,
    text: &str,
    request: &JsonRpcRequest,
) -> io::Result<String> {
    if !super::v1_mounted(&request.method) {
        return serialize(&JsonRpcResponse::error(
            request.id.clone(),
            METHOD_NOT_FOUND,
            "method not mounted on Desktop IPC",
        ));
    }
    let Some(caller) = peer.snapshot().await else {
        return serialize(&JsonRpcResponse::error(
            request.id.clone(),
            AUTH_REQUIRED,
            "IPC owner unavailable",
        ));
    };
    match rpc::prepare(request, &caller, false) {
        Ok(plan) => Ok(executor.execute(text, request, plan, peer, &caller).await),
        Err(error) => serialize(&error),
    }
}

fn serialize(response: &JsonRpcResponse) -> io::Result<String> {
    serde_json::to_string(response).map_err(io::Error::other)
}

// A large Core result is a correlated RPC error, not a transport failure.
fn ipc_response(text: String, id: &Value, synthesis: bool) -> io::Result<String> {
    if response_fits(&text, synthesis.then_some(id)) {
        return Ok(text);
    }
    serialize(&JsonRpcResponse::error(
        Some(id.clone()),
        INTERNAL_ERROR,
        "response exceeds Desktop IPC limits",
    ))
}

fn response_fits(text: &str, synthesis_id: Option<&Value>) -> bool {
    let budget = MAX_FRAME_BYTES as usize;
    match synthesis_id {
        Some(expected) => parse_voice_response_payload(text, budget, |id| id == expected).is_ok(),
        None => text.len() <= budget && parse_payload(text).is_ok(),
    }
}

async fn send(write: &mut (impl AsyncWrite + Unpin), text: &str) -> io::Result<()> {
    timeout(WRITE_TIMEOUT, write_frame(write, text))
        .await
        .map_err(io::Error::other)?
}

#[cfg(test)]
mod tests;
