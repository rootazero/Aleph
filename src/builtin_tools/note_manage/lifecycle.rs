//! Lifecycle surface: `delete` and `rename`.
//!
//! `rename` is the one action that edits notes it was not pointed at — it
//! rewrites every inbound `[[wikilink]]` — so it lives beside `delete` rather
//! than with the content writers.

use tracing::info;

use crate::error::{AlephError, Result};
use crate::memory::notes::sanitize_title;
use crate::memory::notes::store::NoteStore;

use super::args::{NoteManageArgs, NoteManageResult};
use super::NoteManageTool;

impl NoteManageTool {
    pub(super) async fn handle_delete(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args)?;
        let agent_id = agent_id_owned.as_str();

        let category_owned = Self::resolve_category(args, "delete")?;
        let category = category_owned.as_str();
        let filename = args
            .filename
            .as_deref()
            .ok_or_else(|| AlephError::tool("filename is required for delete"))?;

        let safe_filename = sanitize_title(filename)?;
        let file_path = self
            .indexer
            .memory_dir()
            .join(agent_id)
            .join(category)
            .join(format!("{safe_filename}.md"));

        if !file_path.exists() {
            return Err(AlephError::tool(format!(
                "Note '{filename}' in '{category}' does not exist"
            )));
        }

        let note_path = format!("{category}/{safe_filename}");

        // Unified delete path: index rows (incl. embedding) + file, both
        // owned by the indexer.
        self.indexer
            .delete_note(agent_id, category, filename)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to delete note: {e}")))?;

        info!(path = %note_path, "Note deleted");

        Ok(NoteManageResult {
            related_notes: None,
            success: true,
            message: format!("Deleted note '{safe_filename}' from '{category}'"),
            // A delete lands nothing: the note no longer lives anywhere, so a
            // "where it lives" receipt would be a lie in the most literal sense.
            destination: None,
            note_path: Some(note_path),
            content: None,
            notes: None,
            search: None,
        })
    }

    pub(super) async fn handle_rename(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args)?;
        let agent_id = agent_id_owned.as_str();
        let filename = args
            .filename
            .as_deref()
            .ok_or_else(|| AlephError::tool("filename is required for rename"))?;
        let new_title = args
            .new_title
            .as_deref()
            .ok_or_else(|| AlephError::tool("new_title is required for rename"))?;
        let safe_old = sanitize_title(filename)?;
        let safe_new = sanitize_title(new_title)?;
        if safe_old == safe_new {
            return Err(AlephError::tool("new_title equals current filename"));
        }
        // rename_note locates the category itself (find_by_filename); with
        // duplicate filenames across categories it renames the first hit —
        // callers can disambiguate by deleting/recreating instead.
        self.indexer
            .rename_note(agent_id, &safe_old, &safe_new)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to rename note: {e}")))?;
        // Resolve the new category for an honest note_path in the result.
        let new_paths = self
            .indexer
            .store()
            .find_by_filename(&safe_new, agent_id)
            .await
            .unwrap_or_default();
        let note_path = new_paths
            .first()
            .cloned()
            .unwrap_or_else(|| format!("other/{safe_new}"));
        info!(old = %safe_old, new = %safe_new, "Note renamed");
        Ok(NoteManageResult {
            related_notes: None,
            success: true,
            message: format!(
                "Renamed '{safe_old}' → '{safe_new}'. Inbound [[wikilinks]] were rewritten."
            ),
            destination: Some(self.destination(agent_id, &note_path)),
            note_path: Some(note_path),
            content: None,
            notes: None,
            search: None,
        })
    }
}
