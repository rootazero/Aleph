//! Workspace RPC contract — `workspace.{create,list,get,update,archive,unarchive}`.
//!
//! # Why these types live here and not next to the handlers
//!
//! The family has two clients, and they live in two different crates: the CLI
//! (`aleph workspace list|get|create|update|archive|unarchive`) and, since
//! 2026-08-09, the Panel's `/settings/workspaces` page. When this module was
//! written the CLI was the only one, which is why the paragraphs below are
//! about it — the argument is unchanged by the second client, and strengthened:
//! a third place to hand-copy a field name is a third place to drift.
//!
//! `aleph-cli` deliberately **cannot** depend on `alephcore` — its
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

/// Parameters for `workspace.get`, `workspace.archive` and
/// `workspace.unarchive`: an id, nothing else.
///
/// One type for three methods on purpose — they address the same thing, and a
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
/// instead of blanking the column — which is the whole point.
///
/// Since 2026-08-09 the server **builds its response from this type** rather
/// than serializing `AgentEnv` and letting the client ignore the difference.
/// That matters because tolerance and enforcement point opposite ways: a test
/// that parses a real response proves the response is a *superset* of this
/// type, never that it equals it, and four `AgentEnv` fields with no writer and
/// no reader rode the wire for months inside exactly that gap. Both directions
/// are now pinned on the server side, where the field names are owned —
/// `handlers::workspace::the_workspace_responses_carry_the_contract_and_nothing_else`.
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

/// The detail projection of one workspace — what `workspace.get` and
/// `workspace.update` return and `aleph workspace get|update` renders.
///
/// A **second** projection of the server's `AgentEnv` next to [`WorkspaceRow`],
/// on purpose. They answer different questions, and a detail view carrying
/// exactly the list's columns would not have earned the round trip: `profile`,
/// `icon` and `last_active_at` are the reason to ask about one workspace by id.
/// The cost of a second projection is one more reconciliation assertion on the
/// server, not a second place to drift — both are pinned there, against the
/// real `AgentEnv`, in the module that owns the field names.
///
/// Every field below is rendered by a line of `workspace_cmd::detail_pairs`. A
/// field here with no renderer is exactly the defect this module exists to
/// prevent — see [`WorkspaceRow`] for the version of it that shipped and
/// printed a column of dashes for months.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDetail {
    /// Workspace identifier.
    pub id: String,

    /// Display name.
    pub name: String,

    /// Description, `None` when the workspace has none.
    #[serde(default)]
    pub description: Option<String>,

    /// Emoji or icon identifier.
    ///
    /// Defaulted because the server omitted the key entirely when there was
    /// none — it serialized `AgentEnv`, whose `icon` carries
    /// `skip_serializing_if` — so a required field here failed to parse every
    /// icon-less workspace, which is all of them until someone passes
    /// `--icon`. Since 2026-08-09 the server builds the response from *this*
    /// type and sends an explicit `null`, so the default is no longer load
    /// bearing for the current server. It stays for the two readers that still
    /// need it: an older server, and the guarantee that adding
    /// `skip_serializing_if` here later is not a breaking change.
    #[serde(default)]
    pub icon: Option<String>,

    /// Profile the workspace inherits its defaults from.
    pub profile: String,

    /// Creation timestamp. UTC on the wire; rendered in the reader's zone.
    pub created_at: DateTime<Utc>,

    /// Last activity timestamp. UTC on the wire, same as `created_at`.
    ///
    /// Note this is also bumped by a metadata edit, not only by conversation —
    /// `AgentEnvStore::update` writes it.
    pub last_active_at: DateTime<Utc>,

    /// Whether the workspace is archived.
    ///
    /// Always rendered here, unlike the list's conditional `Status` column: a
    /// detail view is reached by exact id and can therefore be about an
    /// archived workspace at any time, so the field can always say two things.
    #[serde(default)]
    pub is_archived: bool,
}

/// Response envelope of `workspace.get` and `workspace.update`.
///
/// One type for both because both wrap the same thing under the same key.
/// `update` additionally sends `"ok": true`, which is deliberately not modelled:
/// a JSON-RPC result IS the success signal, and a client branching on the flag
/// would be asking the same question twice and could disagree with itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEnvelope {
    /// The workspace that was fetched or just updated.
    pub workspace: WorkspaceDetail,
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

    /// An icon-less workspace is the common case. Until 2026-08-09 the server
    /// omitted the key entirely (it serialized its store type, whose `icon`
    /// carries `skip_serializing_if`); it now projects through
    /// [`WorkspaceDetail`] and sends an explicit `null`. A required `icon`
    /// would have failed to parse essentially every real response under the
    /// first shape — the kind of break that shows up only once a client exists
    /// — so the field stays optional-and-defaulted and this test keeps the
    /// **omitted** case, which is the one no longer produced and therefore the
    /// one nothing else would catch if it regressed.
    #[test]
    fn a_detail_parses_a_workspace_that_has_no_icon() {
        let envelope: WorkspaceEnvelope = serde_json::from_value(serde_json::json!({
            "workspace": {
                "id": "crypto",
                "profile": "default",
                "name": "Crypto Trading",
                "description": null,
                "created_at": "2026-08-08T09:30:00Z",
                "last_active_at": "2026-08-08T09:30:00Z",
                "cache_state": { "type": "none" },
                "env_vars": {},
                "is_archived": false,
            }
        }))
        .expect("an icon-less workspace must parse");
        assert!(envelope.workspace.icon.is_none());
        assert_eq!(envelope.workspace.profile, "default");
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
