//! Snapshot conversion from Chrome DevTools MCP tree format.
//!
//! The original AriaSnapshot-based conversion has been replaced by the
//! text-first SnapshotOutput. This module is pending deletion in Task 10.

use super::error::BrowserError;

/// Convert a Chrome DevTools MCP snapshot JSON to text (stub — pending deletion).
#[allow(dead_code)]
pub fn convert_chrome_mcp_snapshot(raw: &serde_json::Value) -> Result<String, BrowserError> {
    Ok(raw.to_string())
}
