//! Opt-in, same-account Desktop Gateway ingress. No legacy network privileges apply.

use std::{
    io,
    net::{IpAddr, Ipv4Addr},
    path::Path,
    time::Duration,
};

use aleph_client::local_ipc::{
    parse_payload, parse_request_payload, parse_voice_response_payload, read_frame_budgeted,
    write_frame, HELLO_TIMEOUT, MAX_FRAME_BYTES, MAX_INFLIGHT,
};
use aleph_protocol::system_hello::{Feature, HelloLimits};
use futures_util::{stream::FuturesUnordered, StreamExt};
use serde_json::Value;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use super::{
    cleanup,
    dispatch::{active_desktop_owner, DispatchPeer},
    event_admission::{next_received, V1Event, V1EventFeed},
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
                    if let Err(error) = serve_peer(read, write, state, stop).await {
                        tracing::debug!(kind = ?error.kind(), "Desktop IPC peer closed");
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
) -> io::Result<()> {
    let Some(store) = state
        .security_store
        .clone()
        .filter(|store| active_desktop_owner(store).is_some())
    else {
        return Ok(());
    };
    let budget = std::sync::Arc::new(Semaphore::new(WIRE_BUDGET));
    let id = format!("ipc-{}", uuid::Uuid::new_v4());
    let hello = tokio::select! {
        biased;
        _ = stop.cancelled() => return Ok(()),
        result = timeout(HELLO_TIMEOUT, read_request(&mut read, &budget)) => result.map_err(io::Error::other)??,
    };
    let Some((text, request, permit)) = hello else {
        return Ok(());
    };
    drop((text, permit));
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
            serve_requests(&mut read, &mut write, &budget, &executor, &peer, &ctx, &mut events).await
        } => result,
    };
    drop((read, write));
    cleanup::run_cleanup(&id, &ctx, &Arc::new(PendingInvokes::new())).await;
    result
}

type FrameBudget = std::sync::Arc<Semaphore>;

async fn read_request(
    reader: &mut (impl AsyncRead + Unpin),
    budget: &FrameBudget,
) -> io::Result<Option<(String, JsonRpcRequest, OwnedSemaphorePermit)>> {
    let Some((text, permit)) = read_frame_budgeted(reader, budget).await? else {
        return Ok(None);
    };
    let request: JsonRpcRequest =
        serde_json::from_value(parse_request_payload(&text)?).map_err(io::Error::other)?;
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
    Ok(Some((text, request, permit)))
}

async fn serve_requests(
    read: &mut (impl AsyncRead + Unpin),
    write: &mut (impl AsyncWrite + Unpin),
    budget: &FrameBudget,
    executor: &rpc::RpcExecutor,
    peer: &DispatchPeer,
    ctx: &ConnectionContext,
    events: &mut Option<V1EventFeed>,
) -> io::Result<()> {
    let mut jobs = FuturesUnordered::new();
    let mut active: Vec<Value> = Vec::new();
    loop {
        // Preserve partial reads while completions are sent. Jobs are polled in
        // the owning task; returning drops all unfinished work before cleanup.
        let frame = read_request(read, budget);
        tokio::pin!(frame);
        let next = loop {
            tokio::select! {
                biased;
                completed = jobs.next(), if !jobs.is_empty() => {
                    if let Some((id, response, voice_synthesis, _permit)) = completed {
                        let id: Value = id;
                        let voice_synthesis: bool = voice_synthesis;
                        let response: String = response?;
                        // Match the receiver's byte and shape limits. A large Core
                        // result is a correlated RPC error, not a transport failure.
                        let response = if response_fits(&response, voice_synthesis.then_some(&id)) {
                            response
                        } else {
                            serialize(&JsonRpcResponse::error(
                                Some(id.clone()),
                                INTERNAL_ERROR,
                                "response exceeds Desktop IPC limits",
                            ))?
                        };
                        send(write, &response).await?;
                        active.retain(|entry| entry != &id);
                    }
                }
                // Live events: admitted in the body, not the branch, so a
                // completion or read winning the race never drops a frame.
                received = next_received(events) => {
                    let feed = events.as_mut().expect("only a feed yields frames");
                    match feed.admit(ctx, received).await {
                        None => {}
                        Some(V1Event::Send(frame)) => send(write, &frame).await?,
                        Some(V1Event::Lost(warning)) => {
                            send(write, &warning).await?;
                            return Ok(());
                        }
                        Some(V1Event::Close(_)) => return Ok(()),
                    }
                }
                request = &mut frame => break request?,
            }
        };
        let Some((text, request, permit)) = next else {
            return Ok(());
        };
        let id = request.id.clone().expect("validated request id");
        if active.contains(&id) || active.len() >= MAX_INFLIGHT as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "IPC request limit exceeded",
            ));
        }
        active.push(id.clone());
        jobs.push(async move {
            let result = execute(executor, peer, &text, &request).await;
            let voice_synthesis =
                request.method == aleph_protocol::panel_voice::VOICE_TURN_SYNTHESIZE_METHOD;
            (id, result, voice_synthesis, permit)
        });
    }
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
