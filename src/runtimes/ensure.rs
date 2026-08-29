//! Capability orchestration — Probe -> Bootstrap -> Register
//!
//! The central function `ensure_capability` is called when a tool
//! needs a runtime that may not be installed.

use crate::error::AlephError;
use crate::runtimes::bootstrap::{self, BootstrapResult};
use crate::runtimes::ledger::{
    now_secs, CapabilityEntry, CapabilityLedger, CapabilitySource, CapabilityStatus,
};
use crate::runtimes::probe;
use crate::sync_primitives::{Arc, AsyncRwLock as RwLock, OnceLock};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{info, warn};

/// Process-global per-capability install lock.
///
/// Serializes the probe→bootstrap section of `ensure_capability` for a single
/// capability name. Without this, two concurrent callers (e.g. two
/// `runtimes.install` RPCs — each spawns its own task — or a CLI run racing a
/// Panel click) both observe `Missing` and run the installer twice, racing on
/// shared install prefixes (`~/.cargo`, `~/.aleph/.venv`, npm `-g`). The
/// status-based guard in the ledger cannot prevent this on its own because the
/// ledger lock is intentionally released across the long `bootstrap::install`
/// await, and `update_status` is a no-op for a not-yet-inserted capability.
///
/// The map is keyed by capability name and never reclaimed — the spec set is a
/// small fixed list, so it is effectively bounded.
fn capability_lock(capability: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<
        crate::sync_primitives::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    > = OnceLock::new();
    let map = LOCKS.get_or_init(|| crate::sync_primitives::Mutex::new(HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .entry(capability.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        // rust-doctor-disable-next-line excessive-clone
        .clone()
}

/// Ensure a capability is ready, probing and bootstrapping if needed.
/// Returns the executable path on success.
///
/// Note: There is an inherent TOCTOU (Time-of-check-to-time-of-use) gap
/// between probing for a binary and the caller using it. This is acceptable
/// for our use case since runtime binaries are rarely deleted while in use.
pub async fn ensure_capability(
    capability: &str,
    ledger: &Arc<RwLock<CapabilityLedger>>,
) -> Result<PathBuf, AlephError> {
    ensure_capability_recursive(capability, ledger, 0).await
}

const MAX_BOOTSTRAP_DEPTH: usize = 10;

// rust-doctor-disable-next-line high-cyclomatic-complexity
async fn ensure_capability_recursive(
    capability: &str,
    ledger: &Arc<RwLock<CapabilityLedger>>,
    depth: usize,
) -> Result<PathBuf, AlephError> {
    if depth > MAX_BOOTSTRAP_DEPTH {
        return Err(AlephError::runtime(
            capability,
            format!(
                "dependency resolution exceeded maximum depth ({MAX_BOOTSTRAP_DEPTH}); \
                 possible circular dependency"
            ),
        ));
    }

    // Fast path: already Ready (use write lock to avoid TOCTOU race)
    {
        let mut guard = ledger.write().await;
        if guard.status(capability) == CapabilityStatus::Ready {
            if let Some(path) = guard.executable(capability) {
                if path.exists() {
                    return Ok(path.to_path_buf());
                }
                // Path gone — mark stale, fall through to re-probe
                warn!(
                    "Capability {} path no longer exists, marking stale",
                    capability
                );
                guard.update_status(capability, CapabilityStatus::Stale);
            }
        }
    }

    // Serialize the probe→bootstrap section per capability. Acquired AFTER the
    // already-Ready fast path above so the common case stays lock-free; held
    // across the whole install so a sibling caller waits here and then exits via
    // the re-check below instead of launching a duplicate installer.
    let cap_lock = capability_lock(capability);
    let _install_guard = cap_lock.lock().await;

    // Probe phase
    info!("Probing for capability: {}", capability);
    {
        let guard = ledger.read().await;
        // Re-check: another task may have resolved this while we waited for the lock
        if guard.status(capability) == CapabilityStatus::Ready {
            if let Some(path) = guard.executable(capability) {
                return Ok(path.to_path_buf());
            }
        }
    }

    let mut probe_result = probe::probe(capability);

    if probe_result.found {
        let bin_path = match probe_result.bin_path.take() {
            Some(path) => path,
            None => {
                return Err(AlephError::other(format!(
                    "Capability {capability} found but no binary path reported"
                )));
            }
        };
        if let Some(ref warning) = probe_result.version_warning {
            warn!("{}", warning);
        }

        let now = now_secs();

        let mut guard = ledger.write().await;
        guard.update(CapabilityEntry {
            name: capability.to_string(),
            // rust-doctor-disable-next-line excessive-clone
            bin_path: bin_path.clone(),
            version: probe_result.version.unwrap_or_default(),
            status: CapabilityStatus::Ready,
            source: probe_result.source,
            last_probed: now,
        });
        if let Err(e) = guard.persist() {
            warn!("Failed to persist ledger after probe success: {}", e);
        }

        info!("Capability {} found at {}", capability, bin_path.display());
        return Ok(bin_path);
    }

    // Bootstrap phase — resolve dependencies first
    for dep in bootstrap::dependencies(capability) {
        Box::pin(ensure_capability_recursive(dep, ledger, depth + 1)).await?;
    }

    // Check if bootstrap spec exists
    if !bootstrap::has_spec(capability) {
        let mut guard = ledger.write().await;
        guard.update_status(capability, CapabilityStatus::Missing);
        return Err(runtime_error(
            capability,
            "not found on PATH and no bootstrap spec available",
            None,
        ));
    }

    info!("Bootstrapping capability: {}", capability);
    {
        let mut guard = ledger.write().await;
        // Re-check after dependency resolution — another task may have finished
        if guard.status(capability) == CapabilityStatus::Ready {
            if let Some(path) = guard.executable(capability) {
                return Ok(path.to_path_buf());
            }
        }
        guard.update_status(capability, CapabilityStatus::Bootstrapping);
    }

    // Run bootstrap (async dispatcher)
    let bootstrap_result = match bootstrap::install(capability).await {
        Ok(result) => result,
        Err(e) => {
            // Reset off `Bootstrapping` so a transient dispatcher error (timeout /
            // I/O / post-install) leaves a terminal state, matching every
            // `BootstrapResult` failure branch below. Without this the `?` early
            // return would strand the entry at `Bootstrapping` indefinitely.
            ledger
                .write()
                .await
                .update_status(capability, CapabilityStatus::Missing);
            return Err(AlephError::runtime(
                capability,
                format!("Bootstrap failed: {e}"),
            ));
        }
    };

    let now = now_secs();

    match bootstrap_result {
        BootstrapResult::Success { bin_path, version } => {
            let mut guard = ledger.write().await;
            guard.update(CapabilityEntry {
                name: capability.to_string(),
                // rust-doctor-disable-next-line excessive-clone
                bin_path: bin_path.clone(),
                version,
                status: CapabilityStatus::Ready,
                source: CapabilitySource::AlephManaged,
                last_probed: now,
            });
            if let Err(e) = guard.persist() {
                warn!("Failed to persist ledger after bootstrap success: {}", e);
            }

            info!(
                "Capability {} bootstrapped at {}",
                capability,
                bin_path.display()
            );
            Ok(bin_path)
        }
        BootstrapResult::PathNotFound { expected } => {
            let mut guard = ledger.write().await;
            guard.update_status(capability, CapabilityStatus::Missing);
            Err(runtime_error(
                capability,
                &format!("installed but binary not found at {expected}"),
                None,
            ))
        }
        BootstrapResult::Failed { stderr } => {
            let mut guard = ledger.write().await;
            guard.update_status(capability, CapabilityStatus::Missing);
            Err(runtime_error(
                capability,
                "bootstrap command returned a non-zero exit code",
                Some(&stderr),
            ))
        }
        BootstrapResult::Unsupported {
            capability: cap,
            reason,
        } => {
            let mut guard = ledger.write().await;
            guard.update_status(capability, CapabilityStatus::Missing);
            Err(runtime_error(
                capability,
                &format!("{cap} is not supported on this platform: {reason}"),
                None,
            ))
        }
        BootstrapResult::UnknownCapability { capability: cap } => {
            let mut guard = ledger.write().await;
            guard.update_status(capability, CapabilityStatus::Missing);
            Err(runtime_error(
                capability,
                &format!("{cap} has no bootstrap spec registered"),
                None,
            ))
        }
    }
}

/// Build a multi-line actionable error message for a failed
/// `ensure_capability` call. Includes the three canonical fix options
/// (CLI, Panel, manual) plus the upstream stderr tail when available.
fn runtime_error(capability: &str, reason: &str, stderr: Option<&str>) -> AlephError {
    use crate::runtimes::find_spec;

    // Prefer a concrete manual-install command (`install_hint`) over the usage
    // description (`llm_hint`): this is the last-resort "Install manually"
    // guidance, so it must say how to *install*, not how to *use*. Falls back to
    // `llm_hint` for any spec without a dedicated install hint.
    let hint = find_spec(capability)
        .and_then(|s| s.install_hint.or(s.llm_hint))
        .unwrap_or("(no hint available — check the runtime's documentation)");

    let stderr_block = stderr
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            let tail = if s.len() > 400 {
                // Safe char-boundary truncation: walk back from len-400 to a valid boundary.
                let mut start = s.len().saturating_sub(400);
                while start > 0 && !s.is_char_boundary(start) {
                    start -= 1;
                }
                &s[start..]
            } else {
                s
            };
            format!("\nStderr tail: {}", tail.trim())
        })
        .unwrap_or_default();

    AlephError::runtime(
        capability,
        format!(
            "Runtime '{capability}' is not available: {reason}{stderr_block}\n\n\
             Fix options:\n  \
               1. Run: aleph-server bootstrap-runtime --only {capability}\n  \
               2. Open Panel → Settings → Runtime and click 'Install'.\n  \
               3. Install manually — {hint}",
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    #[cfg(unix)] // POSIX-only: uses /bin/sh as the pre-populated "ready" binary
    async fn test_ensure_already_ready() {
        let dir = TempDir::new().unwrap();
        let ledger_path = dir.path().join("ledger.json");
        let mut ledger = CapabilityLedger::load_or_create(ledger_path);

        // Pre-populate with a "ready" entry pointing to a real binary
        let bin = PathBuf::from("/bin/sh");
        let now = now_secs();

        ledger.update(CapabilityEntry {
            name: "test-shell".into(),
            bin_path: bin.clone(),
            version: "1.0".into(),
            status: CapabilityStatus::Ready,
            source: CapabilitySource::System,
            last_probed: now,
        });

        let ledger = Arc::new(RwLock::new(ledger));
        let result = ensure_capability("test-shell", &ledger).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), bin);
    }

    #[test]
    fn test_capability_lock_is_per_name_and_stable() {
        // Same capability name must hand back the *same* lock instance — that
        // shared instance is what serializes concurrent installers. Different
        // names must get distinct locks so unrelated installs run in parallel.
        let a1 = capability_lock("alpha");
        let a2 = capability_lock("alpha");
        let b = capability_lock("beta");
        assert!(Arc::ptr_eq(&a1, &a2), "same capability must reuse one lock");
        assert!(
            !Arc::ptr_eq(&a1, &b),
            "distinct capabilities must not share a lock"
        );
    }

    #[tokio::test]
    async fn test_ensure_unknown_capability() {
        let dir = TempDir::new().unwrap();
        let ledger_path = dir.path().join("ledger.json");
        let ledger = CapabilityLedger::load_or_create(ledger_path);
        let ledger = Arc::new(RwLock::new(ledger));

        let result = ensure_capability("totally_unknown_thing_xyz", &ledger).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_failure_message_includes_actionable_hints() {
        let dir = TempDir::new().unwrap();
        let ledger_path = dir.path().join("ledger.json");
        let ledger = CapabilityLedger::load_or_create(ledger_path);
        let ledger = Arc::new(RwLock::new(ledger));

        // Unknown capability takes the no-spec path → actionable error builder.
        let err = ensure_capability("totally_unknown_xyz_for_test", &ledger)
            .await
            .unwrap_err();
        let msg = err.to_string();

        assert!(
            msg.contains("aleph-server bootstrap-runtime"),
            "error should name the CLI remediation, got: {msg}",
        );
        assert!(
            msg.contains("Panel"),
            "error should mention the Panel remediation, got: {msg}",
        );
        assert!(
            msg.contains("totally_unknown_xyz_for_test"),
            "error should reference the failing capability, got: {msg}",
        );
    }
}
