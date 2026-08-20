//! TOML loader for preset and user-defined `FlowSpec` files.
//!
//! See design §5 (TOML shape) and §3.8 (hot reload via `FlowRegistry::replace`).

use crate::sync_primitives::Arc;
use std::path::Path;

use serde::Deserialize;

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_registry::FlowSet;
use crate::orchestrator::flow_spec::FlowSpec;

#[derive(Debug, Deserialize)]
struct FlowFile {
    #[serde(rename = "flow", default)]
    flows: Vec<FlowSpec>,
}

/// Parse the embedded preset catalog. Panics in tests if malformed — the
/// presets are authored and validated at build time.
pub fn load_presets() -> Result<FlowSet, FlowError> {
    let src = include_str!("presets/default_flows.toml");
    parse_flow_file(src).map_err(|e| FlowError::InvalidConfig(format!("presets: {e}")))
}

/// Parse a user flow file (TOML string).
pub fn load_user_flows_from_str(src: &str) -> Result<FlowSet, FlowError> {
    parse_flow_file(src).map_err(|e| FlowError::InvalidConfig(format!("user flow: {e}")))
}

/// Load every `*.toml` under `dir`, merging into a single `FlowSet`.
/// Later files do NOT override earlier ones — duplicates return an error.
pub async fn load_user_flows_from_dir(dir: &Path) -> Result<FlowSet, FlowError> {
    let mut merged = FlowSet::new();
    if !dir.exists() {
        return Ok(merged);
    }
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .map_err(|e| FlowError::InvalidConfig(format!("read {dir:?}: {e}")))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| FlowError::InvalidConfig(format!("iter {dir:?}: {e}")))?
    {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let src = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| FlowError::InvalidConfig(format!("read {path:?}: {e}")))?;
        let parsed = load_user_flows_from_str(&src)?;
        for (id, spec) in parsed {
            // rust-doctor-disable-next-line excessive-clone
            if merged.insert(id.clone(), spec).is_some() {
                return Err(FlowError::InvalidConfig(format!(
                    "duplicate flow id across files: {id}"
                )));
            }
        }
    }
    Ok(merged)
}

fn parse_flow_file(src: &str) -> Result<FlowSet, String> {
    let file: FlowFile = toml::from_str(src).map_err(|e| e.to_string())?;
    let mut out = FlowSet::new();
    for spec in file.flows {
        // rust-doctor-disable-next-line excessive-clone
        let id = spec.id.clone();
        // rust-doctor-disable-next-line excessive-clone
        if out.insert(id.clone(), Arc::new(spec)).is_some() {
            return Err(format!("duplicate flow id: {id}"));
        }
    }
    Ok(out)
}

/// Merge presets + user flows. User flows override presets on id collision.
#[must_use]
pub fn merge_catalogs(presets: FlowSet, user: FlowSet) -> FlowSet {
    let mut out = presets;
    for (id, spec) in user {
        out.insert(id, spec);
    }
    out
}

/// The single answer to "what is the flow catalog for this home directory?".
///
/// Both the boot path (`orchestrator_init::initialize_orchestrator`) and the
/// `gateway.flow.reload` RPC (`gateway::handlers::flow_admin`) compose the
/// catalog through here, and that is the whole point of the function existing:
/// until 2026-08-20 boot called `load_presets()` alone while reload called
/// presets + `load_user_flows_from_dir` + `merge_catalogs`. The two answers
/// disagreed in the direction that is hardest to notice — an operator's
/// `~/.aleph/flows/*.toml` took effect the moment they called reload and
/// vanished on the next restart, with nothing said either time.
///
/// Guarded by `tests/loader.rs::the_catalog_has_exactly_one_composer`, which
/// is a source-level check: once the two callers share this function an
/// equality assertion between them is tautological, so the property worth
/// pinning is that nobody grows a third, hand-rolled composition.
pub async fn load_catalog(flow_dir: &Path) -> Result<FlowSet, FlowError> {
    let presets = load_presets()?;
    let user = load_user_flows_from_dir(flow_dir).await?;
    Ok(merge_catalogs(presets, user))
}
