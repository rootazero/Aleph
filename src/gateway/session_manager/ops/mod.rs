//! Session Manager operations: CRUD, query, compaction, and cleanup methods.

use super::{
    session_type_str, SessionIdentityMeta, SessionManager, SessionManagerError, SessionMetadata,
    SessionPatch, SessionSearchResult, SessionState,
};

mod crud;
mod emit;
mod identity;
mod modify;
mod query;
#[cfg(test)]
mod tests;

pub(crate) use emit::*;

fn map_session_metadata(row: &rusqlite::Row) -> Result<SessionMetadata, rusqlite::Error> {
    let state_str: Option<String> = row.get(8)?;
    let state = state_str
        .and_then(|s| serde_json::from_str(&format!("\"{}\"", s)).ok())
        .unwrap_or_default();
    let metadata_json: Option<String> = row.get(9)?;
    let (topic, status, identity_meta) =
        SessionMetadata::parse_legacy_metadata_json(metadata_json.as_deref());
    Ok(SessionMetadata {
        key: row.get(0)?,
        agent_id: row.get(1)?,
        session_type: row.get(2)?,
        created_at: row.get(3)?,
        last_active_at: row.get(4)?,
        message_count: row.get(5)?,
        total_tokens: row.get(6)?,
        auto_reset_at: row.get(7)?,
        state: Some(state),
        topic,
        status,
        identity_meta,
        label: row.get(10)?,
        input_tokens: row.get(11)?,
        output_tokens: row.get(12)?,
        model: row.get(13)?,
        model_provider: row.get(14)?,
        parent_session_key: row.get(15)?,
        compaction_count: row.get(16)?,
        derived_title: row.get(17).ok(),
        ..Default::default()
    })
}
