//! Capability orchestration — Probe -> Bootstrap -> Register
//!
//! The central function `ensure_capability` is called when a tool
//! needs a runtime that may not be installed.

use crate::error::AlephError;
use crate::runtimes::bootstrap::{self, BootstrapResult};
use crate::runtimes::ledger::{
    CapabilityEntry, CapabilityLedger, CapabilitySource, CapabilityStatus,
};
use crate::runtimes::probe;
use crate::sync_primitives::Arc;
use std::path::PathBuf;
use tokio::sync::RwLock;
use tracing::{info, warn};

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
    // Fast path: already Ready
    {
        let guard = ledger.read().await;
        if guard.status(capability) == CapabilityStatus::Ready {
            if let Some(path) = guard.executable(capability) {
                if path.exists() {
                    return Ok(path.to_path_buf());
                }
                // Path gone — mark stale, fall through to re-probe
                drop(guard);
                warn!(
                    "Capability {} path no longer exists, marking stale",
                    capability
                );
                let mut guard = ledger.write().await;
                guard.update_status(capability, CapabilityStatus::Stale);
            }
        }
    }

    // Probe phase
    info!("Probing for capability: {}", capability);
    {
        let mut guard = ledger.write().await;
        guard.update_status(capability, CapabilityStatus::Probing);
    }

    let probe_result = probe::probe(capability);

    if probe_result.found {
        let bin_path = match probe_result.bin_path.clone() {
            Some(path) => path,
            None => {
                return Err(AlephError::other(format!(
                    "Capability {} found but no binary path reported",
                    capability
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
        Box::pin(ensure_capability(dep, ledger)).await?;
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
        guard.update_status(capability, CapabilityStatus::Bootstrapping);
    }

    // Run bootstrap (async dispatcher)
    let bootstrap_result = bootstrap::install(capability)
        .await
        .map_err(|e| AlephError::runtime(capability, format!("Bootstrap failed: {}", e)))?;

    let now = now_secs();

    match bootstrap_result {
        BootstrapResult::Success { bin_path, version } => {
        let mut guard = ledger.write().await;
        guard.update(CapabilityEntry {
            name: capability.to_string(),
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

    let hint = find_spec(capability)
        .and_then(|s| s.llm_hint)
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
