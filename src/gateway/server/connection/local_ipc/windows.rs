//! Windows listener ownership; admitted connections use the shared RPC loop.

use std::{path::PathBuf, sync::Arc as StdArc};

use aleph_client::local_ipc::windows::PipeAccount;
use tokio::net::windows::named_pipe::NamedPipeServer;
use windows_sys::Win32::Foundation::{ERROR_NO_DATA, ERROR_PIPE_BUSY};

use super::*;

pub(in crate::gateway::server) struct Listener {
    endpoint: PathBuf,
    account: StdArc<PipeAccount>,
    first: NamedPipeServer,
}

pub(in crate::gateway::server) fn bind(path: &Path) -> io::Result<Listener> {
    let account = StdArc::new(PipeAccount::current()?);
    let first = account.create(path, true, MAX_CONNECTIONS)?;
    Ok(Listener {
        endpoint: path.to_owned(),
        account,
        first,
    })
}

async fn replenish(account: &PipeAccount, endpoint: &Path) -> io::Result<NamedPipeServer> {
    let deadline = tokio::time::Instant::now() + HELLO_TIMEOUT;
    loop {
        match account.create(endpoint, false, MAX_CONNECTIONS) {
            // Dropping a Mio pipe cancels overlapped I/O, but the kernel handle
            // can remain alive until its completion is delivered. Callers keep
            // other instances owned during this bounded wait for a free slot.
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            result => return result,
        }
    }
}

pub(in crate::gateway::server) async fn serve(
    listener: Listener,
    state: Arc<GatewaySharedState>,
    stop: CancellationToken,
) -> io::Result<()> {
    let Listener {
        endpoint,
        account,
        first,
    } = listener;
    let mut pending = Some(first);
    let mut tasks: JoinSet<NamedPipeServer> = JoinSet::new();
    let accept = async {
        loop {
            tokio::select! {
                biased;
                Some(completed) = tasks.join_next(), if !tasks.is_empty() => {
                    let retired = match completed {
                        Ok(stream) => stream,
                        Err(error) => break Err(io::Error::other(error)),
                    };
                    // The None case only occurs at the eight-instance cap. Other
                    // handles remain owned by their tasks/results as this one retires.
                    drop(retired);
                    if pending.is_none() {
                        pending = Some(replenish(&account, &endpoint).await?);
                    }
                }
                connected = async { pending.as_ref().unwrap().connect().await }, if pending.is_some() => {
                    let mut stream = pending.take().unwrap();
                    if let Err(error) = connected {
                        if error.raw_os_error() != Some(ERROR_NO_DATA as i32) {
                            break Err(error);
                        }
                        // A client closed between CreateFile and ConnectNamedPipe.
                        // Drop before opening the replacement only if another task
                        // owns an instance (required at the kernel instance cap).
                        let guard = if tasks.is_empty() {
                            Some(stream)
                        } else {
                            drop(stream);
                            None
                        };
                        pending = Some(replenish(&account, &endpoint).await?);
                        drop(guard);
                        continue;
                    }
                    if tasks.len() + 1 < MAX_CONNECTIONS {
                        pending = Some(replenish(&account, &endpoint).await?);
                    }
                    let state = state.clone();
                    let stop = stop.clone();
                    let account = account.clone();
                    tasks.spawn(async move {
                        let authenticated = tokio::select! {
                            biased;
                            _ = stop.cancelled() => false,
                            result = timeout(HELLO_TIMEOUT, account.authenticate_client(&mut stream)) => {
                                matches!(result, Ok(Ok(())))
                            }
                        };
                        let mut discard = !authenticated;
                        let mut cleanup = None;
                        if authenticated {
                            let (read, write) = tokio::io::split(&mut stream);
                            if let Err(error) = serve_peer(read, write, state, stop.clone(), &mut cleanup).await {
                                tracing::debug!(kind = ?error.kind(), "Desktop IPC peer closed");
                                discard = true;
                            }
                        }
                        if discard || stop.is_cancelled() || cleanup.as_ref().is_some_and(|peer| peer.peer.has_offline_voice()) {
                            // Discard unread output on failures/stop so a peer that
                            // never reads cannot pin a pending Windows write.
                            let _ = stream.disconnect();
                        }
                        if let Some(cleanup) = cleanup {
                            cleanup.finish().await;
                        }
                        stream
                    });
                }
            }
        }
    };
    let result = tokio::select! {
        biased;
        _ = stop.cancelled() => Ok(()),
        result = accept => result,
    };
    stop.cancel();
    drop(pending);
    while let Some(completed) = tasks.join_next().await {
        match completed {
            Ok(stream) => drop(stream),
            Err(error) => tracing::warn!(%error, "Desktop IPC connection teardown failed"),
        }
    }
    result
}
