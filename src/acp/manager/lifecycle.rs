//! Session lifecycle: ensure / prompt / cancel / control RPCs / shutdown.

use tracing::{debug, info, warn};

use super::{AcpAdapterManager, SessionEntry, SessionKey};
use crate::acp::adapter::AdapterMode;
use crate::acp::protocol::{AcpErrorCode, AcpOperationError};
use crate::acp::AcpChunkCallback;
use crate::error::Result;
use crate::sync_primitives::Arc;

impl AcpAdapterManager {
    /// Ensure a live ACP session exists for the given `NativeAcp` harness + cwd.
    ///
    /// - If a session exists and is alive, this is a no-op.
    /// - If a session exists but is dead, it is removed and respawned.
    /// - If no session exists, a new one is spawned.
    ///
    /// Only meaningful for `NativeAcp` harnesses; oneshot harnesses don't need sessions.
    ///
    /// Race safety: after spawning (which happens without holding the session lock),
    /// we re-check whether another task already inserted a live session for the same
    /// key. If so, we drop our freshly spawned session instead of overwriting.
    pub async fn ensure_session(&self, harness_id: &str, cwd: &str) -> Result<()> {
        self.ensure_session_named(harness_id, cwd, None).await
    }

    /// Like `ensure_session`, but with an explicit session name so callers
    /// can keep multiple parallel sessions in the same repo
    /// (mirrors acpx's `-s backend` / `-s frontend`). `None` is the
    /// default/unnamed session.
    pub async fn ensure_session_named(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
    ) -> Result<()> {
        let _ = self
            .acquire_live_entry(harness_id, cwd, session_name)
            .await?;
        Ok(())
    }

    /// Look up or spawn a live `SessionEntry` for the given harness + cwd
    /// + optional session name.
    ///
    /// Race-safe: if two tasks call simultaneously, one wins the spawn and
    /// the other's spawned session is killed. A dead/errored entry is
    /// evicted and replaced with a fresh `SessionEntry` (new Arc + new
    /// cancel handle), so callers always get a live session.
    ///
    /// Returned entry's inner session mutex is NOT held — caller may lock it.
    async fn acquire_live_entry(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
    ) -> Result<SessionEntry> {
        let key = SessionKey::with_name(harness_id, cwd, session_name);

        // Fast path: clone the entry under a READ lock, then run the liveness
        // probe WITHOUT holding the map lock. Holding `sessions.write()` across
        // `entry.session.lock().await` would pin the global sessions lock for
        // the entire duration of any in-flight prompt that owns the per-session
        // mutex (up to the request timeout), stalling every other harness/cwd.
        let existing = { self.sessions.read().await.get(&key).cloned() };
        if let Some(entry) = existing {
            let is_live = {
                let mut s = entry.session.lock().await;
                s.is_alive() && s.state() != crate::acp::protocol::AcpSessionState::Error
            };
            if is_live {
                return Ok(entry);
            }
            // Dead — evict, but only if the slot still holds THIS entry; another
            // task may have already respawned a replacement. Notify outside the
            // lock so the (blocking) persistence hook never runs under it.
            let evicted = {
                let mut sessions = self.sessions.write().await;
                match sessions.get(&key) {
                    Some(cur) if Arc::ptr_eq(&cur.session, &entry.session) => {
                        sessions.remove(&key);
                        true
                    }
                    _ => false,
                }
            };
            if evicted {
                warn!(
                    harness_id,
                    "ACP session died or entered error state, respawning"
                );
                self.emit_persistence_event(crate::acp::AcpSessionEvent::Removed {
                    harness_id: harness_id.to_string(),
                    cwd: cwd.to_string(),
                    session_name: session_name.map(str::to_string),
                })
                .await;
            }
        }

        // Slow path: spawn outside any lock, then double-check.
        let harness = {
            let adapters = self.adapters.read().await;
            Arc::clone(adapters.get(harness_id).ok_or_else(|| {
                AcpOperationError::new(
                    AcpErrorCode::HarnessNotFound,
                    format!("Unknown ACP harness: '{harness_id}'"),
                )
            })?)
        };
        let mut new_session = harness.spawn_session(Some(cwd)).await?;

        let mut sessions = self.sessions.write().await;
        if let Some(existing) = sessions.get(&key).cloned() {
            // Race lost — drop our extra session.
            debug!(
                harness_id,
                "ACP session race: another task spawned first, dropping ours"
            );
            new_session.kill().await;
            return Ok(existing);
        }

        info!(harness_id, "ACP session started");
        let entry = SessionEntry::new(new_session);
        sessions.insert(key, entry.clone());
        Ok(entry)
    }

    /// Look up an existing `SessionEntry` WITHOUT spawning a new subprocess.
    ///
    /// Returns `Ok(Some(entry))` if a live entry exists in the pool,
    /// `Ok(None)` if no entry is registered for this (harness, cwd, name)
    /// tuple, and an error only if the pool lookup itself fails.
    ///
    /// `set_mode` / `set_model` / `set_config_option` / `authenticate` use
    /// this instead of `acquire_live_entry` so that a missing session does
    /// NOT silently leak a child process that the next call to
    /// `require_session_id` is going to reject anyway (see ACP-01).
    async fn lookup_existing_entry(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
    ) -> Result<Option<SessionEntry>> {
        let key = SessionKey::with_name(harness_id, cwd, session_name);
        let existing = self.sessions.read().await.get(&key).cloned();
        let Some(entry) = existing else {
            return Ok(None);
        };
        let is_live = {
            let mut s = entry.session.lock().await;
            s.is_alive() && s.state() != crate::acp::protocol::AcpSessionState::Error
        };
        if !is_live {
            // Evict the dead entry so the next caller (or a `prompt_named`
            // with `reuse_session: false`) starts cleanly. Match the eviction
            // policy of `acquire_live_entry` AND mirror its Removed emit so
            // the panel sees a consistent lifecycle when set_mode / set_model /
            // set_config_option / authenticate find a dead pool entry
            // (ACP-R4-04).
            let mut sessions = self.sessions.write().await;
            let evicted = match sessions.get(&key) {
                Some(cur) if Arc::ptr_eq(&cur.session, &entry.session) => {
                    sessions.remove(&key);
                    true
                }
                _ => false,
            };
            drop(sessions);
            if evicted {
                self.emit_persistence_event(crate::acp::AcpSessionEvent::Removed {
                    harness_id: harness_id.to_string(),
                    cwd: cwd.to_string(),
                    session_name: session_name.map(str::to_string),
                })
                .await;
            }
            return Ok(None);
        }
        Ok(Some(entry))
    }

    /// Send a prompt to the specified harness, using the appropriate mode.
    ///
    /// - **`NativeAcp`**: Extracts session from pool (brief lock), uses it, re-inserts if alive.
    /// - **Oneshot**: Spawns a fresh process, waits for output.
    ///
    /// `mode`: Override the harness default mode. `None` uses `harness.mode()`.
    /// `reuse_session`: If `false`, kills any existing session and starts fresh.
    /// `on_chunk`: Streaming callback (wired in Task 7).
    pub async fn prompt(
        &self,
        harness_id: &str,
        prompt_text: &str,
        cwd: &str,
        mode: Option<AdapterMode>,
        reuse_session: bool,
        on_chunk: Option<AcpChunkCallback>,
    ) -> Result<String> {
        self.prompt_named(
            harness_id,
            prompt_text,
            cwd,
            None,
            mode,
            reuse_session,
            on_chunk,
        )
        .await
    }

    /// Like `prompt`, but with an explicit session name. `None` is the
    /// default/unnamed session and is equivalent to `prompt()`.
    #[allow(clippy::too_many_arguments)]
    pub async fn prompt_named(
        &self,
        harness_id: &str,
        prompt_text: &str,
        cwd: &str,
        session_name: Option<&str>,
        mode: Option<AdapterMode>,
        reuse_session: bool,
        on_chunk: Option<AcpChunkCallback>,
    ) -> Result<String> {
        // Resolve effective mode and validate
        let (effective_mode, timeout) = {
            let adapters = self.adapters.read().await;
            let harness = adapters.get(harness_id).ok_or_else(|| {
                AcpOperationError::new(
                    AcpErrorCode::HarnessNotFound,
                    format!("Unknown ACP harness: '{harness_id}'"),
                )
            })?;

            let effective = mode.unwrap_or_else(|| harness.mode());

            // Validate mode is supported
            if !harness.supported_modes().contains(&effective) {
                return Err(AcpOperationError::new(
                    AcpErrorCode::ModeUnsupported,
                    format!("Harness '{harness_id}' does not support {effective:?} mode"),
                )
                .into());
            }

            let timeout = harness.build_config(Some(cwd)).timeout;
            (effective, timeout)
        };
        // harnesses read lock dropped here

        match effective_mode {
            AdapterMode::NativeAcp => {
                let key = SessionKey::with_name(harness_id, cwd, session_name);

                // If not reusing, evict any existing session first so the
                // next acquire_live_entry spawns a fresh one.
                if !reuse_session {
                    // Bind the removed entry first so the `sessions` write guard
                    // is dropped at the `;` — an `if let` scrutinee would keep it
                    // alive across `lock()`/`kill()` (edition-2021 temporary
                    // lifetime), pinning the whole map while we kill the child.
                    let old = self.sessions.write().await.remove(&key);
                    if let Some(old) = old {
                        let mut s = old.session.lock().await;
                        s.kill().await;
                    }
                }

                let entry = self
                    .acquire_live_entry(harness_id, cwd, session_name)
                    .await?;

                // Lock the inner mutex — concurrent prompts to the same
                // (harness, cwd) queue here. The cancel handle (held outside
                // this lock by the manager) can still fire `session/cancel`.
                let mut session = entry.session.lock().await;

                let result = session
                    .prompt(prompt_text, cwd, timeout, on_chunk.as_ref())
                    .await;

                match result {
                    Ok((text, _notifications)) => {
                        // Read liveness + id, then release the per-session mutex
                        // BEFORE emitting — the persistence hook performs blocking
                        // disk I/O and must never run under the session lock
                        // (matches the Removed paths above and the Err arm below).
                        let alive = session.is_alive();
                        let sid = if alive {
                            session.acp_session_id()
                        } else {
                            None
                        };
                        drop(session);
                        if alive {
                            if let Some(sid) = sid {
                                // Idempotent Created: ensure_session can't fire
                                // it because session_id is None at spawn time.
                                self.emit_persistence_event(crate::acp::AcpSessionEvent::Created {
                                    harness_id: harness_id.to_string(),
                                    acp_session_id: sid,
                                    cwd: cwd.to_string(),
                                    session_name: session_name.map(str::to_string),
                                })
                                .await;
                            }
                        } else {
                            // Process died — evict the entry only if it is still
                            // the same session we locked (a concurrent caller may
                            // have already respawned a replacement).
                            if self.remove_if_same(&key, &entry).await {
                                self.emit_persistence_event(crate::acp::AcpSessionEvent::Removed {
                                    harness_id: harness_id.to_string(),
                                    cwd: cwd.to_string(),
                                    session_name: session_name.map(str::to_string),
                                })
                                .await;
                                warn!(harness_id, "ACP session died after prompt, evicted");
                            }
                        }
                        Ok(text)
                    }
                    Err(e) => {
                        // Only kill + evict on retryable errors
                        // (`SessionDead`, `Timeout`, `SpawnFailed`). Application-
                        // level rejections (`ModeUnsupported`,
                        // `SessionControlUnsupported`, `AuthRequired`, ...) are
                        // NOT a sign the subprocess is broken — leave it alive
                        // so the next retry can succeed without respawning.
                        // The `From<AcpOperationError> for AlephError` impl
                        // preserves the `is_retryable()` bit on
                        // `AlephError::AcpError { retryable, .. }`, so we
                        // read it back here without downcasting. See ACP-04.
                        let retryable = match &e {
                            crate::error::AlephError::AcpError { retryable, .. } => *retryable,
                            _ => false,
                        };
                        if retryable && session.is_alive() {
                            session.kill().await;
                        }
                        drop(session);
                        // Only remove the entry if it is still the same session
                        // we used; a concurrent caller may have respawned it.
                        if retryable && self.remove_if_same(&key, &entry).await {
                            self.emit_persistence_event(crate::acp::AcpSessionEvent::Removed {
                                harness_id: harness_id.to_string(),
                                cwd: cwd.to_string(),
                                session_name: session_name.map(str::to_string),
                            })
                            .await;
                        }
                        Err(e)
                    }
                }
            }
            AdapterMode::Oneshot => {
                // Clone Arc ref under brief read lock; execute_oneshot may take minutes.
                let harness = {
                    let adapters = self.adapters.read().await;
                    Arc::clone(adapters.get(harness_id).ok_or_else(|| {
                        AcpOperationError::new(
                            AcpErrorCode::HarnessNotFound,
                            format!("Unknown ACP harness: '{harness_id}'"),
                        )
                    })?)
                };
                harness.execute_oneshot(prompt_text, cwd).await
            }
        }
    }

    /// Cooperatively cancel the in-flight prompt on the specified harness + cwd.
    ///
    /// Uses the pre-cloned `CancelHandle` so the cancel notification can
    /// race ahead of an in-flight prompt that currently holds the
    /// per-session mutex. The agent will respond with a `stopReason`
    /// notification, the prompt resolves, and the next caller sees the
    /// session in `Idle` state again.
    ///
    /// Returns `SessionDead` if no entry exists for this harness/cwd.
    pub async fn cancel(&self, harness_id: &str, cwd: &str) -> Result<()> {
        self.cancel_named(harness_id, cwd, None).await
    }

    /// Like `cancel`, but targets a specific named session.
    pub async fn cancel_named(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
    ) -> Result<()> {
        let key = SessionKey::with_name(harness_id, cwd, session_name);
        let entry = self
            .sessions
            .read()
            .await
            .get(&key)
            .cloned()
            .ok_or_else(|| {
                AcpOperationError::new(
                    AcpErrorCode::SessionDead,
                    format!(
                        "No active ACP session for '{}' in '{}'{}",
                        harness_id,
                        cwd,
                        session_name
                            .map(|n| format!(" (session '{n}')"))
                            .unwrap_or_default()
                    ),
                )
            })?;
        entry.cancel.send_cancel().await
    }

    /// Remove `key` from the session map only if the stored entry still refers
    /// to the same `SessionEntry` we used. This prevents a concurrent
    /// `acquire_live_entry` respawn from being evicted after we dropped the
    /// per-session lock.
    async fn remove_if_same(&self, key: &SessionKey, entry: &SessionEntry) -> bool {
        let mut sessions = self.sessions.write().await;
        match sessions.get(key) {
            Some(cur) if Arc::ptr_eq(&cur.session, &entry.session) => {
                sessions.remove(key);
                true
            }
            _ => false,
        }
    }

    /// Run a session-control RPC against the existing session for
    /// (harness, cwd). Returns `SessionDead` if the session is gone.
    /// Inner errors are normalized to `SessionControlUnsupported` when
    /// the adapter doesn't implement the method.
    pub async fn set_mode(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
        mode_id: &str,
    ) -> Result<()> {
        let (entry, timeout) = self
            .entry_and_timeout(harness_id, cwd, session_name)
            .await?;
        let mut s = entry.session.lock().await;
        s.set_mode(mode_id, timeout).await
    }

    pub async fn set_model(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
        model_id: &str,
    ) -> Result<()> {
        let (entry, timeout) = self
            .entry_and_timeout(harness_id, cwd, session_name)
            .await?;
        let mut s = entry.session.lock().await;
        s.set_model(model_id, timeout).await
    }

    pub async fn set_config_option(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
        key: &str,
        value: serde_json::Value,
    ) -> Result<()> {
        let (entry, timeout) = self
            .entry_and_timeout(harness_id, cwd, session_name)
            .await?;
        let mut s = entry.session.lock().await;
        s.set_config_option(key, value, timeout).await
    }

    /// Authenticate the existing session. `credential` is opaque — typically
    /// pulled from env `ACP_AUTH_<METHOD_ID>` by the caller.
    pub async fn authenticate(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
        method_id: &str,
        credential: &str,
    ) -> Result<()> {
        let (entry, timeout) = self
            .entry_and_timeout(harness_id, cwd, session_name)
            .await?;
        let mut s = entry.session.lock().await;
        s.authenticate(method_id, credential, timeout).await
    }

    async fn entry_and_timeout(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
    ) -> Result<(SessionEntry, std::time::Duration)> {
        let timeout = {
            let adapters = self.adapters.read().await;
            let harness = adapters.get(harness_id).ok_or_else(|| {
                AcpOperationError::new(
                    AcpErrorCode::HarnessNotFound,
                    format!("Unknown ACP harness: '{harness_id}'"),
                )
            })?;
            harness.build_config(Some(cwd)).timeout
        };
        // Use `lookup_existing_entry` (NOT `acquire_live_entry`) so a missing
        // session returns `Ok(None)` instead of silently spawning a fresh
        // subprocess that the next `require_session_id` is going to reject.
        // See ACP-01.
        let entry = self
            .lookup_existing_entry(harness_id, cwd, session_name)
            .await?
            .ok_or_else(|| {
                AcpOperationError::new(
                    AcpErrorCode::SessionDead,
                    format!(
                        "ACP session for harness '{harness_id}' (cwd '{cwd}') was not started; \
                         call prompt_named first"
                    ),
                )
            })?;
        Ok((entry, timeout))
    }

    /// Shut down a single named session. Cancels any in-flight prompt,
    /// removes the entry from the pool, kills the subprocess, then emits
    /// `Removed` so panels and persistence stay in sync. Idempotent — no
    /// error when the session is already gone.
    pub async fn shutdown_named(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
    ) -> Result<()> {
        let key = SessionKey::with_name(harness_id, cwd, session_name);

        // Fire cancel first so the agent gets a chance to flush partial
        // output before we evict. Best-effort — a missing session is fine.
        let cancel_handle = self
            .sessions
            .read()
            .await
            .get(&key)
            .map(|entry| entry.cancel.clone());
        if let Some(handle) = cancel_handle {
            // Best-effort pre-fire so the harness can shut down its turn
            // cleanly, but log if the cancel channel is already closed or
            // the receiver has been dropped — the caller otherwise sees
            // `Ok(())` while the agent keeps running, which is exactly
            // the silent failure that makes "why won't it stop?" tickets
            // undebuggable.
            if let Err(e) = handle.send_cancel().await {
                tracing::warn!(
                    error = %e,
                    "shutdown_named: pre-fire cancel failed; agent may not stop promptly"
                );
            }
        }

        // Now evict + kill. Drop the write lock before locking the inner
        // session mutex to avoid holding both at once.
        let entry = self.sessions.write().await.remove(&key);
        if let Some(entry) = entry {
            let mut s = entry.session.lock().await;
            s.kill().await;
            drop(s);
            self.emit_persistence_event(crate::acp::AcpSessionEvent::Removed {
                harness_id: harness_id.to_string(),
                cwd: cwd.to_string(),
                session_name: session_name.map(str::to_string),
            })
            .await;
        }
        Ok(())
    }

    /// Kill all active sessions.
    pub async fn shutdown_all(&self) {
        let entries: Vec<(SessionKey, SessionEntry)> = {
            let mut sessions = self.sessions.write().await;
            sessions.drain().collect::<Vec<_>>()
        };
        for (key, _) in &entries {
            info!(harness_id = %key.harness_id, cwd = ?key.cwd, "Shutting down ACP session");
        }
        // Best-effort pre-fire cancel on every entry before the kill pass.
        // Mirrors `shutdown_named` so global shutdown gives in-flight prompts
        // a chance to flush partial output instead of interrupting them
        // mid-stream (ACP-R4-05). A failed cancel is logged but never
        // blocks the teardown.
        futures::future::join_all(entries.iter().map(|(_, entry)| async move {
            if let Err(e) = entry.cancel.send_cancel().await {
                tracing::debug!(
                    error = %e,
                    "shutdown_all: pre-fire cancel failed; proceeding to kill"
                );
            }
        }))
        .await;
        // Small grace period for the cancel notification to flush before
        // the kill pass interrupts the harness's stdin/stdout pipes.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Kill all sessions concurrently. A single wedged session
        // (e.g. one whose child is stuck in uninterruptible IO) used
        // to block every other session's teardown behind it because
        // the kill loop was sequential. With `join_all` the wall-clock
        // cost is bounded by the slowest session, not the sum.
        futures::future::join_all(entries.iter().map(|(_, entry)| async move {
            let mut session = entry.session.lock().await;
            session.kill().await;
        }))
        .await;
        for (key, _) in entries {
            self.emit_persistence_event(crate::acp::AcpSessionEvent::Removed {
                harness_id: key.harness_id,
                cwd: key.cwd.to_string_lossy().into_owned(),
                session_name: if key.name.is_empty() {
                    None
                } else {
                    Some(key.name)
                },
            })
            .await;
        }
    }
}
