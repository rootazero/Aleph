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

pub(crate) use crud::NewMessage;
pub(crate) use emit::*;

/// The `sessions` column list every `SELECT` that feeds [`map_session_metadata`]
/// must use, in the order that mapper decodes positionally.
///
/// One constant rather than five hand-copied lists. A positional decoder plus N
/// hand-written `SELECT`s is a contract with no compiler behind it: adding a
/// column to one of them turns another one's `row.get(K)` into a RUNTIME type
/// error, and the five copies here had already drifted in whitespace. Both
/// mappers (this one and the `SessionStore` impl's) read the same order because
/// there is now only one mapper.
pub(crate) const SESSION_COLUMNS: &str =
    "key, agent_id, session_type, created_at, last_active_at, \
     message_count, total_tokens, auto_reset_at, state, metadata, \
     label, input_tokens, output_tokens, model, model_provider, \
     parent_session_key, compaction_count, derived_title, \
     estimated_cost_usd, owner_user_id, scope_id, last_message_preview";

pub(crate) fn map_session_metadata(
    row: &rusqlite::Row,
) -> Result<SessionMetadata, rusqlite::Error> {
    let state_str: Option<String> = row.get(8)?;
    let state = state_str
        .and_then(|s| serde_json::from_str(&format!("\"{s}\"")).ok())
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
        // Legacy rows: ALTER TABLE ADD COLUMN without DEFAULT leaves NULL for
        // pre-migration data. Coerce to 0 so historical sessions load instead
        // of panicking on `Invalid column type Null` (mirrors the
        // sqlite_backend mapper).
        input_tokens: row.get::<_, Option<i64>>(11)?.unwrap_or(0),
        output_tokens: row.get::<_, Option<i64>>(12)?.unwrap_or(0),
        model: row.get(13)?,
        model_provider: row.get(14)?,
        parent_session_key: row.get(15)?,
        compaction_count: row.get(16)?,
        derived_title: row.get(17).ok(),
        // Column added later; `.ok()` + default keeps a pre-migration row (or a
        // NULL) reading as 0.0 rather than panicking.
        estimated_cost_usd: row.get::<_, Option<f64>>(18).ok().flatten().unwrap_or(0.0),
        // P1 columns; `.ok()` keeps a pre-migration row reading as `None`
        // (legacy, adoption-by-absence) rather than panicking.
        owner_user_id: row.get(19).ok(),
        scope_id: row.get(20).ok(),
        // Column added later, same `.ok()` treatment. Until it existed this
        // field had a writer only in the FILE backend, so a
        // `session_store_backend = "sqlite"` install rendered a null preview on
        // `sessions.preview` and on every `sessions` tool row, forever —
        // indistinguishable from "this conversation has no messages yet".
        last_message_preview: row.get(21).ok().flatten(),
        ..Default::default()
    })
}
