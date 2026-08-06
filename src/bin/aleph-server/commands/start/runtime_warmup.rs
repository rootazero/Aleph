//! Background-spawned runtime capability probe (Bun / Python / browser / …).
//!
//! Fires from `start_server` via `tokio::spawn` and writes the result ledger
//! to `~/.aleph/runtimes/ledger.json`. Failure is best-effort: a missing or
//! unwritable runtimes dir simply emits a warning and the call returns.

pub(super) async fn runtime_startup_warmup() {
    use alephcore::runtimes::{
        self,
        ledger::{migrate_from_legacy, CapabilityEntry},
        CapabilityLedger, CapabilityStatus, SPECS,
    };
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::RwLock;

    let runtimes_dir = match runtimes::get_runtimes_dir() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "runtime warmup skipped: cannot resolve runtimes dir");
            return;
        }
    };
    if let Err(e) = tokio::fs::create_dir_all(&runtimes_dir).await {
        tracing::warn!(error = %e, "runtime warmup skipped: cannot create runtimes dir");
        return;
    }
    let ledger_path = runtimes_dir.join("ledger.json");
    let ledger: Arc<RwLock<CapabilityLedger>> = match migrate_from_legacy(&runtimes_dir) {
        Ok(ledger) => Arc::new(RwLock::new(ledger)),
        Err(e) => {
            tracing::warn!(
                "Legacy manifest migration failed: {e}, falling back to ledger load_or_create"
            );
            Arc::new(RwLock::new(CapabilityLedger::load_or_create(&ledger_path)))
        }
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();

    let mut missing = Vec::new();
    for spec in SPECS {
        let result = runtimes::probe::probe(spec.name);
        let mut g = ledger.write().await;
        if result.found {
            g.update(CapabilityEntry {
                name: spec.name.into(),
                bin_path: result.bin_path.unwrap_or_default(),
                version: result.version.unwrap_or_default(),
                status: CapabilityStatus::Ready,
                source: result.source,
                last_probed: now,
            });
        } else if runtimes::supported_on_current_os(spec.name) {
            // mark_missing (not update_status) so a stale path/version from a
            // ledger copied off another machine is cleared, not persisted.
            g.mark_missing(spec.name);
            missing.push(spec.name);
        }
    }
    if let Err(e) = ledger.write().await.persist() {
        tracing::warn!("Failed to persist runtime ledger: {}", e);
    }
    if missing.is_empty() {
        tracing::info!("runtime warmup: all capabilities ready");
    } else {
        tracing::warn!(
            missing = ?missing,
            "runtime capabilities missing — browser / python tools will fail until installed. \
             Run 'aleph-server bootstrap-runtime' or open Panel → Settings → Runtime.",
        );
    }
}

#[cfg(test)]
mod warmup_tests {
    use super::runtime_startup_warmup;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_warmup_runs_and_persists_ledger() {
        let dir = TempDir::new().unwrap();
        std::env::set_var("HOME", dir.path());

        runtime_startup_warmup().await;

        let ledger_path = dir.path().join(".aleph/runtimes/ledger.json");
        assert!(
            ledger_path.exists(),
            "ledger must be persisted at {}",
            ledger_path.display()
        );
        let content = tokio::fs::read_to_string(&ledger_path).await.unwrap();
        let _: serde_json::Value =
            serde_json::from_str(&content).expect("ledger must be valid JSON");
    }
}
