//! Bounded filesystem admission, prepared off-lock and committed by the lifecycle writer.

use std::sync::Arc;

use tokio::io::AsyncReadExt;

use super::{matches_integrity, PanelAssetPublicationStore};
use crate::extension::panel::{
    derive_panel_declaration, derive_panel_snapshot, PanelProjectionDiagnostic,
};
use crate::extension::plugin_trust::OwnerTrustPolicy;
use crate::extension::types::PluginRecord;

const ENTRY_LIMIT: usize = 4 * 1024 * 1024;
const GENERATION_LIMIT: usize = 64 * 1024 * 1024;

pub(in crate::extension) struct PreparedPanelPublication {
    candidates: Vec<(PluginRecord, Arc<[u8]>)>,
    diagnostics: Vec<PanelProjectionDiagnostic>,
}

impl PreparedPanelPublication {
    pub(in crate::extension) async fn prepare(
        records: Vec<PluginRecord>,
        policy: &OwnerTrustPolicy,
    ) -> Self {
        let mut projection = derive_panel_snapshot(&records, policy, 0);
        // The shared snapshot already orders canonical keys for deterministic budget admission.
        let mut candidates = Vec::new();
        let mut total = 0;
        for declaration in projection.snapshot.declarations {
            let record = records
                .iter()
                .find(|record| {
                    record.id == declaration.descriptor.key.plugin_id
                        && record.version.as_deref() == Some(declaration.plugin_version.as_str())
                        && record
                            .panel
                            .as_ref()
                            .is_some_and(|panel| panel.tab_id == declaration.descriptor.key.tab_id)
                })
                .expect("a derived declaration belongs to its source records");
            match read_entry(record).await {
                Ok(bytes) if total + bytes.len() <= GENERATION_LIMIT => {
                    total += bytes.len();
                    candidates.push((record.clone(), bytes));
                }
                Ok(_) => projection.diagnostics.push(PanelProjectionDiagnostic {
                    plugin_id: record.id.clone(),
                    code: "panel.asset_generation_limit",
                }),
                Err(code) => projection.diagnostics.push(PanelProjectionDiagnostic {
                    plugin_id: record.id.clone(),
                    code,
                }),
            }
        }
        Self {
            candidates,
            diagnostics: projection.diagnostics,
        }
    }
}

impl PanelAssetPublicationStore {
    pub(in crate::extension) fn commit(
        &mut self,
        prepared: PreparedPanelPublication,
        policy: &OwnerTrustPolicy,
    ) -> Vec<PanelProjectionDiagnostic> {
        let mut diagnostics = prepared.diagnostics;
        let Some(revision) = self.revision.checked_add(1).filter(|_| !self.unavailable) else {
            self.records.clear();
            self.unavailable = true;
            diagnostics.push(PanelProjectionDiagnostic {
                plugin_id: String::new(),
                code: "panel.revision_exhausted",
            });
            return diagnostics;
        };
        let mut publications = Vec::new();
        for (record, bytes) in prepared.candidates {
            // Policy may have changed during I/O; caller holds the current policy read lock here.
            match derive_panel_declaration(&record, policy, revision) {
                Ok(Some(declaration)) => publications.push((declaration, bytes, record.scope_key)),
                Ok(None) => {}
                Err(error) => diagnostics.push(PanelProjectionDiagnostic {
                    plugin_id: record.id,
                    code: error.code(),
                }),
            }
        }
        if self.replace(revision, publications).is_err() {
            self.records.clear();
            self.revision = revision;
            self.unavailable = true;
            diagnostics.push(PanelProjectionDiagnostic {
                plugin_id: String::new(),
                code: "panel.publication_unavailable",
            });
        }
        diagnostics
    }
}

async fn read_entry(record: &PluginRecord) -> Result<Arc<[u8]>, &'static str> {
    let panel = record
        .panel
        .as_ref()
        .expect("declaration admission requires an explicit Panel manifest");
    let root = tokio::fs::canonicalize(&record.root_dir)
        .await
        .map_err(|_| "panel.asset_unavailable")?;
    let entry = tokio::fs::canonicalize(root.join(&panel.entry))
        .await
        .map_err(|_| "panel.asset_unavailable")?;
    if !entry.starts_with(&root) {
        return Err("panel.asset_outside_root");
    }
    let metadata = tokio::fs::metadata(&entry)
        .await
        .map_err(|_| "panel.asset_unavailable")?;
    if !metadata.is_file() {
        return Err("panel.asset_not_file");
    }
    if metadata.len() > ENTRY_LIMIT as u64 {
        return Err("panel.asset_entry_limit");
    }
    let file = tokio::fs::File::open(entry)
        .await
        .map_err(|_| "panel.asset_unavailable")?;
    let mut bytes = Vec::new();
    file.take((ENTRY_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "panel.asset_unavailable")?;
    if bytes.len() > ENTRY_LIMIT {
        return Err("panel.asset_entry_limit");
    }
    if !matches_integrity(&bytes, &panel.asset_integrity) {
        return Err("panel.asset_integrity_mismatch");
    }
    Ok(bytes.into())
}
