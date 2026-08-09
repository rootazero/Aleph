//! The one place a [`TaskError`] becomes a JSON-RPC error code.
//!
//! Both task services (cron, heartbeat) reach the wire through here. Before
//! this existed, every `cron.*` and `heartbeat.*` handler wrote its own
//! `JsonRpcResponse::error(id, INTERNAL_ERROR, format!("Failed to …: {e}"))`,
//! which meant a chain to a job that does not exist, an unknown job id and a
//! failed disk write all arrived as `-32603 Internal error`. That code is a
//! statement about the *server*: it tells a client to retry, tells an operator
//! to read the server log, and says nothing about the one thing that was
//! actually true — the caller could have fixed it.
//!
//! Keeping the mapping here rather than on `TaskError` itself keeps the
//! transport out of `src/tasks/` (R4: the domain does not know what JSON-RPC
//! is), and keeping it in *one* function rather than one per handler is what
//! makes the guard below possible: a handler that reintroduces a bare
//! `INTERNAL_ERROR` next to a service call is a source-level failure, not
//! something you have to notice in a code review.
//!
//! The three-way split matches `handlers::projects::project_error_response`,
//! which reached the same answer for `ProjectError`. Two subsystems agreeing
//! by coincidence is how a third one gets it wrong, so: **not-found is
//! `RESOURCE_NOT_FOUND`, anything the caller can fix by editing the request is
//! `INVALID_PARAMS`, and `INTERNAL_ERROR` is reserved for our own failures.**

use crate::gateway::protocol::{
    JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, RESOURCE_NOT_FOUND,
};
use crate::tasks::shared::error::TaskError;

/// Render a task-service failure as the JSON-RPC error it actually is.
///
/// `context` is the verb-shaped prefix the handlers already used ("Failed to
/// create job") so the message a client sees does not change — only the code,
/// which is the half that was lying.
pub fn respond(id: Option<serde_json::Value>, context: &str, error: &TaskError) -> JsonRpcResponse {
    let code = match error {
        // The addressed job/task is absent. `cron.*` and `heartbeat.*` are
        // operator-gated, so there is no existence oracle to protect here and
        // a named 404 is the honest answer (contrast `visibility::
        // not_found_response`, where not-found is *also* the refusal shape).
        TaskError::NotFound(_) => RESOURCE_NOT_FOUND,
        // Well-formed request, refused content: a chain to a missing job, a
        // cycle, a schedule that can never fire, a job in a state that
        // declines the verb. The caller changes the request and retries.
        TaskError::Invalid(_) => INVALID_PARAMS,
        // Ours.
        TaskError::Internal(_) => INTERNAL_ERROR,
    };
    JsonRpcResponse::error(id, code, format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_of(error: &TaskError) -> i32 {
        respond(Some(serde_json::json!(1)), "Failed to do the thing", error)
            .error
            .expect("must be an error response")
            .code
    }

    #[test]
    fn each_variant_gets_its_own_code() {
        assert_eq!(
            code_of(&TaskError::not_found("job", "abc")),
            RESOURCE_NOT_FOUND
        );
        assert_eq!(code_of(&TaskError::invalid("cycle")), INVALID_PARAMS);
        assert_eq!(code_of(&TaskError::internal("disk full")), INTERNAL_ERROR);
    }

    /// The message a client reads must be unchanged by the typing — the point
    /// of this round was the code, and a silently reworded message would break
    /// the Panel strings that quote it.
    #[test]
    fn the_message_keeps_the_shape_handlers_used_to_write() {
        let response = respond(
            Some(serde_json::json!(1)),
            "Failed to create job",
            &TaskError::invalid("chain target not found: b"),
        );
        assert_eq!(
            response.error.unwrap().message,
            "Failed to create job: chain target not found: b"
        );
    }

    /// Neither task handler may spell `INTERNAL_ERROR` itself.
    ///
    /// Source-level because at runtime a handler that folds a caller error into
    /// `-32603` is indistinguishable from one that correctly reported an
    /// internal failure — you only see it by reading the classification, which
    /// is exactly what nobody does. The two files' remaining error responses
    /// are parameter-parsing rejections (`INVALID_PARAMS`, decided before any
    /// service call) and this function.
    ///
    /// `\r` is stripped before splitting: the repo's Windows checkout is CRLF,
    /// where a separator anchored to `\n` matches nothing and the "production
    /// prefix" silently becomes the whole file, test module included — at which
    /// point this test would be satisfied by its own assertion strings.
    #[test]
    fn no_task_handler_writes_an_internal_error_code_of_its_own() {
        const HANDLERS: [(&str, &str); 2] = [
            (
                "src/gateway/handlers/cron/real.rs",
                include_str!("cron/real.rs"),
            ),
            (
                "src/gateway/handlers/heartbeat.rs",
                include_str!("heartbeat.rs"),
            ),
        ];

        let mut checked = 0usize;
        for (path, src) in HANDLERS {
            let src = src.replace('\r', "");
            let production = src.split("#[cfg(test)]").next().unwrap_or(&src);
            assert!(
                production.contains("task_error::respond"),
                "{path} no longer routes task-service failures through the one \
                 classifier — it has either grown its own mapping or dropped \
                 the wiring"
            );
            assert!(
                !production.contains("INTERNAL_ERROR"),
                "{path} spells INTERNAL_ERROR directly. A task-service failure \
                 must go through `task_error::respond`, which decides between \
                 RESOURCE_NOT_FOUND / INVALID_PARAMS / INTERNAL_ERROR; a bare \
                 -32603 tells the caller the server broke when usually it did not"
            );
            checked += 1;
        }
        assert_eq!(checked, HANDLERS.len());
    }
}
