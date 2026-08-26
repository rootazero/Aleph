//! The one place a [`CanvasError`] becomes a JSON-RPC error code.
//!
//! Same shape and same reasoning as `handlers::task_error` (which the module
//! doc there spells out at length): `Result<_, String>` — or a per-handler
//! `match` — is how a stale revision, an unknown canvas id and a failed disk
//! write all arrive as `-32603 Internal error`, telling the caller the server
//! broke when usually it did not. Keeping the mapping in one function is what
//! makes the source-level guard below possible.
//!
//! The classification is the house three-way split plus the one code this
//! subsystem is built around:
//!
//! - not-found → `RESOURCE_NOT_FOUND` — and on this family not-found is ALSO
//!   the refusal shape: `handlers::canvas` answers a canvas the caller may
//!   not see with the byte-identical message a missing id produces (the
//!   no-oracle contract, `projects::project_not_found` precedent).
//! - caller-fixable → `INVALID_PARAMS`.
//! - stale `base_revision` → [`REVISION_CONFLICT`] — its own code, because a
//!   conflict is neither the caller's mistake nor ours: the message carries
//!   the current revision and the Panel/model branch on the CODE to re-pull
//!   and replay (the consumer and the code were born in the same change).
//! - ours → `INTERNAL_ERROR`.

use crate::canvas::CanvasError;
use crate::gateway::protocol::{
    JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, RESOURCE_NOT_FOUND, REVISION_CONFLICT,
};

/// Render a canvas-store failure as the JSON-RPC error it actually is.
///
/// `context` is the verb-shaped prefix ("Failed to apply canvas ops"), so the
/// message stays the familiar `"{context}: {error}"` shape every other throat
/// in this directory produces.
pub fn respond(
    id: Option<serde_json::Value>,
    context: &str,
    error: &CanvasError,
) -> JsonRpcResponse {
    let code = match error {
        CanvasError::NotFound(_) => RESOURCE_NOT_FOUND,
        CanvasError::Invalid(_) => INVALID_PARAMS,
        // The Display of `Conflict` names the current revision — the number
        // both the Panel's replay path and the model's self-heal need, kept
        // in the message so no consumer has to grow a data-field parser.
        CanvasError::Conflict { .. } => REVISION_CONFLICT,
        CanvasError::Internal(_) => INTERNAL_ERROR,
    };
    JsonRpcResponse::error(id, code, format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_of(error: &CanvasError) -> i32 {
        respond(Some(serde_json::json!(1)), "Failed to do the thing", error)
            .error
            .expect("must be an error response")
            .code
    }

    #[test]
    fn each_variant_gets_its_own_code() {
        assert_eq!(
            code_of(&CanvasError::NotFound("canvas cv-x".into())),
            RESOURCE_NOT_FOUND
        );
        assert_eq!(
            code_of(&CanvasError::Invalid("too many ops".into())),
            INVALID_PARAMS
        );
        assert_eq!(
            code_of(&CanvasError::Conflict {
                current_revision: 7
            }),
            REVISION_CONFLICT
        );
        assert_eq!(
            code_of(&CanvasError::Internal("disk full".into())),
            INTERNAL_ERROR
        );
    }

    /// The conflict message must carry the current revision as a number the
    /// caller can read back — it is the whole point of the code: re-pull,
    /// replay, resend against THIS revision.
    #[test]
    fn the_conflict_message_names_the_current_revision() {
        let response = respond(
            Some(serde_json::json!(1)),
            "Failed to apply canvas ops",
            &CanvasError::Conflict {
                current_revision: 42,
            },
        );
        let err = response.error.expect("error response");
        assert_eq!(err.code, REVISION_CONFLICT);
        assert!(
            err.message.contains("42"),
            "conflict message must name the current revision: {}",
            err.message
        );
    }

    /// No canvas handler may spell `INTERNAL_ERROR` itself.
    ///
    /// Source-level because at runtime a handler that folds a caller error
    /// into `-32603` is indistinguishable from one that correctly reported an
    /// internal failure. `\r` is stripped before splitting: on a CRLF
    /// checkout a separator anchored to `\n` matches nothing and the
    /// "production prefix" silently becomes the whole file, test module
    /// included — at which point this test would be satisfied by its own
    /// assertion strings (§10).
    #[test]
    fn no_canvas_handler_writes_an_internal_error_code_of_its_own() {
        let src = include_str!("canvas.rs").replace('\r', "");
        let production = crate::utils::source_scan::production_prefix(&src);
        // Self-check that the prefix really is the production half, not an
        // empty or test-only slice: the handler family must be in it.
        assert!(
            production.contains("pub async fn handle_"),
            "the scanned production prefix of handlers/canvas.rs contains no \
             handler — the split marker moved and this guard is scanning air"
        );
        assert!(
            production.contains("canvas_error::respond"),
            "handlers/canvas.rs no longer routes store failures through the \
             one classifier — it has either grown its own mapping or dropped \
             the wiring"
        );
        assert!(
            !production.contains("INTERNAL_ERROR"),
            "handlers/canvas.rs spells INTERNAL_ERROR directly. A canvas \
             failure must go through `canvas_error::respond`, which decides \
             between RESOURCE_NOT_FOUND / INVALID_PARAMS / REVISION_CONFLICT \
             / INTERNAL_ERROR; a bare -32603 tells the caller the server \
             broke when usually it did not"
        );
    }
}
