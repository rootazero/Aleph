use std::sync::Arc;

use aleph_protocol::jsonrpc::{RESOURCE_NOT_FOUND, REVISION_CONFLICT};
use aleph_protocol::memory_graph::{
    GraphMutateNoteParams, GraphMutateNoteResponse, NoteAddress, NoteMutation, NoteMutationStatus,
    PartialNoteMutation,
};

use crate::gateway::caller_identity::current_caller_user;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::visibility::partition_visible_to;
use crate::memory::notes::indexer::{CheckedNoteAction, CheckedNoteError, NoteIndexer};
use crate::memory::store::sqlite::SqliteMemoryBackend;

pub(crate) fn note_mutation_partition_allowed(partition: &str, actor: Option<&str>) -> bool {
    actor.is_some_and(|actor| !actor.is_empty() && partition_visible_to(partition, Some(actor)))
}

pub(crate) async fn handle_mutate_note(
    req: JsonRpcRequest,
    indexer: Arc<NoteIndexer<SqliteMemoryBackend>>,
) -> JsonRpcResponse {
    let params: GraphMutateNoteParams = match req.params.map(serde_json::from_value).transpose() {
        Ok(Some(params)) => params,
        _ => return JsonRpcResponse::error(req.id, INVALID_PARAMS, "Invalid note mutation"),
    };
    let mut address = params.address;
    if address.agent_id.trim().is_empty()
        || address.node_id.trim().is_empty()
        || params.revision.len() != 64
        || !params.revision.bytes().all(|byte| byte.is_ascii_hexdigit())
        || params.idempotency_key.trim().is_empty()
    {
        return JsonRpcResponse::error(req.id, INVALID_PARAMS, "Invalid note mutation");
    }
    let actor = current_caller_user();
    if !note_mutation_partition_allowed(&address.agent_id, actor.as_deref()) {
        return JsonRpcResponse::error(req.id, RESOURCE_NOT_FOUND, "Note not found");
    }
    let (action, status) = match params.action {
        NoteMutation::Update { content } => {
            if let Err(error) =
                crate::builtin_tools::note_manage::scan_note_for_exfiltration(&content)
            {
                return JsonRpcResponse::error(req.id, INVALID_PARAMS, error.to_string());
            }
            (
                CheckedNoteAction::Update { content },
                NoteMutationStatus::Updated,
            )
        }
        NoteMutation::Rename { title } => (
            CheckedNoteAction::Rename { title },
            NoteMutationStatus::Renamed,
        ),
        NoteMutation::Delete => (CheckedNoteAction::Delete, NoteMutationStatus::Deleted),
    };
    match indexer
        .mutate_note_checked(
            &address.agent_id,
            &address.node_id,
            &params.revision,
            action,
        )
        .await
    {
        Ok(node_id) => {
            address.node_id = node_id;
            JsonRpcResponse::success(
                req.id,
                serde_json::json!(GraphMutateNoteResponse { address, status }),
            )
        }
        Err(CheckedNoteError::NotFound) => {
            JsonRpcResponse::error(req.id, RESOURCE_NOT_FOUND, "Note not found")
        }
        Err(CheckedNoteError::Conflict) => JsonRpcResponse::error(
            req.id,
            REVISION_CONFLICT,
            "Note changed. Reload it before retrying.",
        ),
        Err(CheckedNoteError::DestinationExists) => JsonRpcResponse::error(
            req.id,
            REVISION_CONFLICT,
            "A note already exists at the destination",
        ),
        Err(CheckedNoteError::Invalid(error)) => {
            JsonRpcResponse::error(req.id, INVALID_PARAMS, error.to_string())
        }
        Err(CheckedNoteError::Partial {
            address: node_id,
            failed_source,
            source,
        }) => {
            tracing::warn!(error = %source, "Note mutation needs index reconciliation");
            address.node_id = node_id;
            let failed_source = failed_source.map(|node_id| NoteAddress {
                agent_id: address.agent_id.clone(),
                node_id,
            });
            JsonRpcResponse::error_with_data(
                req.id,
                INTERNAL_ERROR,
                "Note changed on disk. Index reconciliation is required.",
                serde_json::json!(PartialNoteMutation {
                    reconciliation_required: true,
                    address,
                    failed_source,
                }),
            )
        }
        Err(CheckedNoteError::Failed(error)) => {
            tracing::warn!(%error, "Note mutation failed");
            JsonRpcResponse::error(req.id, INTERNAL_ERROR, "Could not update note")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::memory::notes::{indexer::note_revision, store::NoteStore};
    use serde_json::json;

    #[tokio::test]
    async fn foreign_partition_mutation_leaves_disk_and_index_untouched() {
        const PARTITION: &str = "main__u-alice";
        const ADDRESS: &str = "reference/private";
        const CONTENT: &str = "---\ncategory: reference\n---\n\nOriginal note\n";
        let (_root, memory_dir) = crate::utils::scratch::scratch_root();
        let (_db_root, db) = super::super::test_helpers::make_db();
        let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), db.clone()));
        indexer
            .write_note_raw(PARTITION, "reference", "private", CONTENT)
            .await
            .unwrap();
        let before = db
            .get_note_index(ADDRESS, PARTITION)
            .await
            .unwrap()
            .unwrap();
        let request = |node_id, id| {
            JsonRpcRequest::with_id(
                aleph_protocol::memory_graph::GRAPH_MUTATE_NOTE_METHOD,
                Some(json!({
                    "address": {"agent_id": PARTITION, "node_id": node_id},
                    "revision": note_revision(CONTENT),
                    "action": {"type": "delete"},
                    "idempotency_key": "fixture"
                })),
                json!(id),
            )
        };
        let denied = CALLER_USER
            .scope(
                Some("u-bob".into()),
                handle_mutate_note(request(ADDRESS, 1), indexer.clone()),
            )
            .await;
        let missing = CALLER_USER
            .scope(
                Some("u-alice".into()),
                handle_mutate_note(request("reference/missing", 2), indexer.clone()),
            )
            .await;
        let denied = denied.error.unwrap();
        let missing = missing.error.unwrap();
        assert_eq!(denied.code, RESOURCE_NOT_FOUND);
        assert_eq!(denied.code, missing.code);
        assert_eq!(denied.message, missing.message);
        assert_eq!(
            tokio::fs::read_to_string(memory_dir.join(PARTITION).join(format!("{ADDRESS}.md")))
                .await
                .unwrap(),
            CONTENT
        );
        let after = db
            .get_note_index(ADDRESS, PARTITION)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before.content_hash, after.content_hash);
    }
}
