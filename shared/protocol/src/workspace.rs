//! Workspace RPC contract — `workspace.{create,list,get,update,archive}`.
//!
//! # Why these types live here and not next to the handlers
//!
//! The family's only client is the CLI (`aleph workspace list|create|archive`),
//! and `aleph-cli` deliberately **cannot** depend on `alephcore` — its
//! `Cargo.toml` says so in capitals, because it doubles as the reference
//! implementation of the protocol. So before this module the wire shape was
//! written twice, once as a `#[derive(Deserialize)]` in the handler and once as
//! a `serde_json::json!` literal in the CLI, with nothing connecting them.
//!
//! They disagreed. The handler required `id`; the CLI sent `name`. Every
//! `aleph workspace create` and every `aleph workspace archive` ever run came
//! back `INVALID_PARAMS` — the commands had not worked once since they were
//! written, and no test went red, because the CLI's own tests asserted a JSON
//! literal against itself:
//!
//! ```ignore
//! let params = serde_json::json!({ "name": "test-ws" });
//! assert_eq!(params["name"], "test-ws");   // cannot fail; asserts nothing
//! ```
//!
//! Sharing one type makes a rename a **compile** error on both sides. That is
//! the only guard that holds for a contract with a client in another crate: a
//! grep or a hand-copied fixture can be fooled by a field name that merely
//! looks right, and a tautological assertion is fooled by everything.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Parameters for `workspace.create`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCreateParams {
    /// Workspace identifier — the addressable key, URL-safe slug (`"crypto"`).
    ///
    /// This is what `workspace.get` / `workspace.archive` take, so it is also
    /// what a list has to show: an id the caller cannot read back is an id
    /// they cannot archive.
    pub id: String,

    /// Human-readable display name.
    ///
    /// The store defaults this to the id when a row is inserted; a client that
    /// has no display name to offer should send the id rather than an empty
    /// string, so the two sides never disagree about what the default is.
    pub name: String,

    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Optional emoji or icon identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Parameters for `workspace.list`. Optional as a whole — a request with no
/// params at all is the default view.
///
/// `deny_unknown_fields` because the only field here is a request to *widen*
/// what comes back: a misspelled key would otherwise deserialize to `false`
/// and answer a narrower question than the caller asked, with no way to tell
/// that from a genuinely empty result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceListParams {
    /// Include archived workspaces in the response.
    ///
    /// The store has always taken this flag; the handler hard-coded `false`
    /// until 2026-08-08, which left an archived workspace unreachable from
    /// every client — `archive` is a soft delete whose result nothing could
    /// show.
    #[serde(default)]
    pub include_archived: bool,
}

/// Parameters for `workspace.get` and `workspace.archive`: an id, nothing else.
///
/// One type for two methods on purpose — they address the same thing, and a
/// second struct would be a second place to drift.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRef {
    /// Workspace identifier.
    pub id: String,
}

/// Parameters for `workspace.update`. Every field but `id` is a patch: absent
/// means "leave it alone", not "clear it".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceUpdateParams {
    /// Workspace identifier.
    pub id: String,

    /// New display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// New description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// New icon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// One row of `workspace.list`, as a client renders it.
///
/// A deliberate **projection** of the server's `AgentEnv`, not a copy of it:
/// this carries exactly the fields something prints. A field here with no
/// renderer is precisely the defect this module exists to fix — the CLI's
/// table read `status` and `created`, neither of which the server has ever
/// emitted (it has `is_archived` and `created_at`), so every row printed a
/// column of dashes and looked merely empty.
///
/// `is_archived` earned its place only once [`WorkspaceListParams`] existed:
/// while the handler hard-coded `list(false)` every row was active by
/// construction, so a status column could carry exactly one value. It is
/// rendered in the `--include-archived` view and nowhere else.
///
/// Unknown fields are ignored, so the server may add fields freely. A *rename*
/// of one of these is a breaking change, and deserialization then fails loudly
/// instead of blanking the column — which is the whole point. The projection is
/// pinned against the real `AgentEnv` by a test on the server side, where the
/// source of truth lives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRow {
    /// Workspace identifier — the key `workspace.archive` takes.
    pub id: String,

    /// Display name.
    pub name: String,

    /// Description, `None` when the workspace has none.
    #[serde(default)]
    pub description: Option<String>,

    /// Creation timestamp. UTC on the wire; a human-facing column renders it
    /// in the reader's zone.
    pub created_at: DateTime<Utc>,

    /// Whether the workspace is archived.
    ///
    /// Defaulted rather than required: the default view asks for active rows
    /// only, and a server that omits the field there is not wrong.
    #[serde(default)]
    pub is_archived: bool,
}

/// Response envelope of `workspace.list`.
///
/// `workspaces` has no serde default on purpose: a response that omits the key
/// is a protocol error, and reading it as "zero workspaces" would turn a broken
/// server into an empty-looking-but-fine list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceList {
    /// The visible workspaces. Archived rows appear only when the request
    /// carried `include_archived` — see [`WorkspaceListParams`].
    pub workspaces: Vec<WorkspaceRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_optionals_are_omitted_rather_than_sent_as_null() {
        let params = WorkspaceCreateParams {
            id: "crypto".to_string(),
            name: "crypto".to_string(),
            description: None,
            icon: None,
        };
        assert_eq!(
            serde_json::to_value(&params).unwrap(),
            serde_json::json!({ "id": "crypto", "name": "crypto" })
        );
    }

    #[test]
    fn a_list_response_without_the_key_is_an_error_not_an_empty_list() {
        let err = serde_json::from_value::<WorkspaceList>(serde_json::json!({}));
        assert!(
            err.is_err(),
            "a missing `workspaces` key must not read as zero workspaces"
        );
    }

    #[test]
    fn a_misspelled_list_flag_is_rejected_rather_than_read_as_false() {
        // The failure this prevents is silent: `include_arcived` would
        // deserialize to the default and return the active-only view, which is
        // indistinguishable from "you have no archived workspaces".
        assert!(serde_json::from_value::<WorkspaceListParams>(
            serde_json::json!({ "include_arcived": true })
        )
        .is_err());
        assert!(
            serde_json::from_value::<WorkspaceListParams>(serde_json::json!({}))
                .expect("an empty object is the default view")
                .include_archived
                .eq(&false)
        );
    }

    #[test]
    fn a_row_ignores_fields_it_does_not_render() {
        let row: WorkspaceRow = serde_json::from_value(serde_json::json!({
            "id": "crypto",
            "name": "Crypto Trading",
            "description": null,
            "created_at": "2026-08-08T09:00:00Z",
            "env_vars": { "SECRET": "x" },
            "allowed_tools": ["a"],
        }))
        .expect("extra server-side fields must not break a client");
        assert_eq!(row.id, "crypto");
        assert_eq!(row.name, "Crypto Trading");
        assert!(row.description.is_none());
    }
}
