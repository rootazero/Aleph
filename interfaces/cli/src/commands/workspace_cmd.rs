//! Workspace management commands
//!
//! The request/response shapes are `aleph_protocol::workspace::*` — the same
//! types the server deserializes. They are not re-declared here on purpose;
//! see that module for what happened the last time they were.

use chrono::TimeZone;
use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliError, CliResult};
use aleph_protocol::workspace::{
    WorkspaceCreateParams, WorkspaceDetail, WorkspaceEnvelope, WorkspaceList, WorkspaceListParams,
    WorkspaceRef, WorkspaceRow, WorkspaceUpdateParams,
};

/// Columns of `aleph workspace list`.
///
/// `ID` comes first because it is the only cell the other two subcommands
/// accept: a list that showed just the display name left `archive` with
/// nothing to copy.
const LIST_HEADERS: &[&str] = &["ID", "Name", "Description", "Created"];

/// Columns of `aleph workspace list --include-archived`.
///
/// `Status` exists in this view only. In the default view every row is active
/// by construction, and a column that can hold one value is not information —
/// the original `Status` column was worse than that, printing a dash per row
/// because it read a field name the server does not have.
const LIST_HEADERS_WITH_STATUS: &[&str] = &["ID", "Name", "Description", "Created", "Status"];

/// The headers matching `include_archived`.
fn headers(include_archived: bool) -> &'static [&'static str] {
    if include_archived {
        LIST_HEADERS_WITH_STATUS
    } else {
        LIST_HEADERS
    }
}

/// Render one row in `tz`. `-` here means the server sent no description, which
/// is a fact about the workspace — unlike the previous `-`, which meant this CLI
/// was reading a field the server does not have.
///
/// The timezone is a parameter rather than a hardcoded `Local` so the rendering
/// can be asserted against a fixed offset: a test that converted the expectation
/// the same way the code does would agree with any offset, including a wrong one.
/// Callers pass `Local` — `created_at` is UTC on the wire, and a bare UTC clock
/// in a human-facing column reads as a wrong time, not as another timezone.
/// `--json` still carries the exact RFC 3339 instant.
fn row_cells<Tz: TimeZone>(workspace: &WorkspaceRow, tz: &Tz, include_archived: bool) -> Vec<String>
where
    Tz::Offset: std::fmt::Display,
{
    let mut cells = vec![
        workspace.id.clone(),
        workspace.name.clone(),
        workspace
            .description
            .clone()
            .unwrap_or_else(|| "-".to_string()),
        workspace
            .created_at
            .with_timezone(tz)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
    ];
    if include_archived {
        cells.push(
            if workspace.is_archived {
                "archived"
            } else {
                "active"
            }
            .to_string(),
        );
    }
    cells
}

/// Render one workspace as the labelled lines `aleph workspace get|update`
/// prints.
///
/// This is the renderer [`WorkspaceDetail`]'s doc points at: every field of
/// that struct appears below exactly once, which is the property that keeps a
/// field from being added to the wire and then quietly displayed nowhere.
///
/// Timestamps take `tz` for the same reason [`row_cells`] does — a test that
/// converted its expectation the way the code does would agree with a wrong
/// offset as readily as a right one.
fn detail_pairs<Tz: TimeZone>(workspace: &WorkspaceDetail, tz: &Tz) -> Vec<(&'static str, String)>
where
    Tz::Offset: std::fmt::Display,
{
    let stamp = |at: &chrono::DateTime<chrono::Utc>| {
        at.with_timezone(tz).format("%Y-%m-%d %H:%M").to_string()
    };
    let or_dash = |value: &Option<String>| value.clone().unwrap_or_else(|| "-".to_string());

    vec![
        ("ID", workspace.id.clone()),
        ("Name", workspace.name.clone()),
        ("Description", or_dash(&workspace.description)),
        ("Icon", or_dash(&workspace.icon)),
        ("Profile", workspace.profile.clone()),
        ("Created", stamp(&workspace.created_at)),
        ("Last active", stamp(&workspace.last_active_at)),
        (
            "Status",
            if workspace.is_archived {
                "archived"
            } else {
                "active"
            }
            .to_string(),
        ),
    ]
}

/// Build the `workspace.update` params, refusing a patch that patches nothing.
///
/// A `workspace.update` carrying only an id is accepted by the server and
/// changes nothing, so without this the CLI would report success for a command
/// that did not happen — indistinguishable, to the person who mistyped
/// `--nmae`, from a change that silently failed to stick. The check is here
/// rather than in the handler because clap is where the omission is visible as
/// an omission; the server cannot tell "no fields" from "a patch that happens
/// to be empty".
fn update_params(
    id: &str,
    name: Option<&str>,
    description: Option<&str>,
    icon: Option<&str>,
) -> CliResult<WorkspaceUpdateParams> {
    if name.is_none() && description.is_none() && icon.is_none() {
        return Err(CliError::Other(format!(
            "nothing to update for workspace '{id}': pass at least one of \
             --name, --description, --icon"
        )));
    }

    Ok(WorkspaceUpdateParams {
        id: id.to_string(),
        name: name.map(str::to_string),
        description: description.map(str::to_string),
        icon: icon.map(str::to_string),
    })
}

/// Build the `workspace.create` params.
///
/// `name` defaults to the id, mirroring `AgentEnvStore::create`, so the two
/// sides cannot disagree about what an omitted display name means.
fn create_params(
    id: &str,
    name: Option<&str>,
    description: Option<&str>,
    icon: Option<&str>,
) -> WorkspaceCreateParams {
    WorkspaceCreateParams {
        id: id.to_string(),
        name: name.unwrap_or(id).to_string(),
        description: description.map(str::to_string),
        icon: icon.map(str::to_string),
    }
}

/// List all workspaces
pub async fn list(
    server_url: &str,
    config: &CliConfig,
    include_archived: bool,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client
        .call(
            "workspace.list",
            Some(WorkspaceListParams { include_archived }),
        )
        .await?;

    // `--json` is a raw passthrough and must not be gated on this CLI being
    // able to render the payload; only the table needs the projection, and
    // there a shape it cannot read is an error worth showing rather than a
    // table of dashes.
    let rows: Vec<Vec<String>> = if json {
        Vec::new()
    } else {
        serde_json::from_value::<WorkspaceList>(result.clone())?
            .workspaces
            .iter()
            .map(|workspace| row_cells(workspace, &chrono::Local, include_archived))
            .collect()
    };

    output::print_table(headers(include_archived), &rows, json, &result);

    client.close().await?;
    Ok(())
}

/// Create a new workspace
pub async fn create(
    server_url: &str,
    config: &CliConfig,
    id: &str,
    name: Option<&str>,
    description: Option<&str>,
    icon: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = create_params(id, name, description, icon);
    let result: Value = client.call("workspace.create", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Workspace '{id}' created.");
    }

    client.close().await?;
    Ok(())
}

/// Show one workspace in detail.
///
/// Reaches archived workspaces too — the server answers this by exact id and
/// reports `is_archived`, so the Status line says which it is. That is the
/// half of "readable, not writable" this command owns; [`update`] owns the
/// other.
pub async fn get(server_url: &str, config: &CliConfig, id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = WorkspaceRef { id: id.to_string() };
    let result: Value = client.call("workspace.get", Some(params)).await?;

    render_detail(&result, json)?;

    client.close().await?;
    Ok(())
}

/// Change a workspace's name, description or icon.
///
/// Omitted fields are left alone rather than cleared (the server COALESCEs).
/// An archived workspace is refused — `get` can still show it, and [`unarchive`]
/// is the way back. That refusal is deliberate rather than a gap: restoring a
/// workspace is its own verb, not something a rename does on the side.
pub async fn update(
    server_url: &str,
    config: &CliConfig,
    id: &str,
    name: Option<&str>,
    description: Option<&str>,
    icon: Option<&str>,
    json: bool,
) -> CliResult<()> {
    // Before connecting: an empty patch is a mistake at the command line, and
    // there is nothing for a round trip to add.
    let params = update_params(id, name, description, icon)?;

    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let result: Value = client.call("workspace.update", Some(params)).await?;

    render_detail(&result, json)?;

    client.close().await?;
    Ok(())
}

/// Print a `workspace.get`/`workspace.update` envelope.
///
/// `--json` is a raw passthrough and must not be gated on this CLI being able
/// to project the payload — same rule [`list`] follows, and the same reason: a
/// shape the table cannot read is worth surfacing as an error rather than as a
/// screen of dashes.
fn render_detail(result: &Value, json: bool) -> CliResult<()> {
    let pairs = if json {
        Vec::new()
    } else {
        let envelope: WorkspaceEnvelope = serde_json::from_value(result.clone())?;
        detail_pairs(&envelope.workspace, &chrono::Local)
    };

    output::print_detail(&pairs, json, result);
    Ok(())
}

/// Archive a workspace
pub async fn archive(server_url: &str, config: &CliConfig, id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = WorkspaceRef { id: id.to_string() };
    let result: Value = client.call("workspace.archive", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Workspace '{id}' archived.");
    }

    client.close().await?;
    Ok(())
}

/// Restore an archived workspace — the inverse of [`archive`].
///
/// Prints the restored workspace in full rather than a one-line confirmation,
/// which is why this reuses [`render_detail`] and [`archive`] does not: the
/// server returns the row (its response is `get`/`update`'s envelope), and the
/// thing the operator wants confirmed is that the workspace they meant is back
/// — name, description and a Status line that now says `active`.
///
/// Nothing is created. The ID was never released while archived, so this is a
/// flag coming off a row that stayed where it was, with the workspace's memory
/// and notes untouched on disk under the same ID.
pub async fn unarchive(
    server_url: &str,
    config: &CliConfig,
    id: &str,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = WorkspaceRef { id: id.to_string() };
    let result: Value = client.call("workspace.unarchive", Some(params)).await?;

    render_detail(&result, json)?;

    client.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_display_name_defaults_to_the_id() {
        let params = create_params("test-ws", None, None, None);
        assert_eq!(params.id, "test-ws");
        assert_eq!(params.name, "test-ws");
        assert!(params.description.is_none());
        assert!(params.icon.is_none());
    }

    #[test]
    fn an_explicit_name_never_becomes_the_id() {
        // The id is the addressable key; a display name with spaces must not
        // silently become the thing `archive` has to be given.
        let params = create_params("crypto", Some("Crypto Trading"), Some("notes"), Some("💰"));
        assert_eq!(params.id, "crypto");
        assert_eq!(params.name, "Crypto Trading");
        assert_eq!(params.description.as_deref(), Some("notes"));
        assert_eq!(params.icon.as_deref(), Some("💰"));
    }

    /// The regression itself: what goes on the wire must carry `id`. This
    /// asserts the serialized request, not a literal built next to it — the
    /// test it replaces compared `json!({"name": …})["name"]` to `"test-ws"`,
    /// which is true no matter what the server expects.
    #[test]
    fn the_create_request_carries_the_id_the_server_addresses_by() {
        let wire = serde_json::to_value(create_params("test-ws", None, None, None)).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({ "id": "test-ws", "name": "test-ws" })
        );

        let wire = serde_json::to_value(WorkspaceRef {
            id: "test-ws".to_string(),
        })
        .unwrap();
        assert_eq!(wire, serde_json::json!({ "id": "test-ws" }));
    }

    #[test]
    fn a_row_renders_every_column_from_a_real_response_body() {
        let list: WorkspaceList = serde_json::from_value(serde_json::json!({
            "workspaces": [{
                "id": "crypto",
                "profile": "default",
                "name": "Crypto Trading",
                "description": null,
                "created_at": "2026-08-08T09:30:00Z",
                "last_active_at": "2026-08-08T09:30:00Z",
                "cache_state": { "type": "none" },
                "is_archived": false,
            }]
        }))
        .expect("a real workspace.list body must parse");

        assert_eq!(
            row_cells(&list.workspaces[0], &chrono::Utc, false),
            vec!["crypto", "Crypto Trading", "-", "2026-08-08 09:30"],
        );
    }

    /// Both views, because the table now has two shapes and only one of them
    /// is exercised by a default `workspace list`.
    #[test]
    fn every_header_has_a_cell_in_both_views() {
        let row = WorkspaceRow {
            id: "crypto".to_string(),
            name: "Crypto Trading".to_string(),
            description: None,
            created_at: "2026-08-08T09:30:00Z".parse().expect("valid timestamp"),
            is_archived: false,
        };

        for include_archived in [false, true] {
            assert_eq!(
                headers(include_archived).len(),
                row_cells(&row, &chrono::Utc, include_archived).len(),
                "a header with no cell (or the reverse) misaligns every row \
                 (include_archived = {include_archived})"
            );
        }
    }

    /// The `Status` column exists only where it can say two things. It was
    /// dropped entirely when `workspace.list` hard-coded `list(false)`, and it
    /// comes back with `--include-archived` and not before.
    #[test]
    fn status_is_shown_only_in_the_view_that_can_contain_archived_rows() {
        let archived = WorkspaceRow {
            id: "old".to_string(),
            name: "old".to_string(),
            description: None,
            created_at: "2026-08-08T09:30:00Z".parse().expect("valid timestamp"),
            is_archived: true,
        };
        let active = WorkspaceRow {
            is_archived: false,
            ..archived.clone()
        };

        assert!(!headers(false).contains(&"Status"));
        assert_eq!(row_cells(&archived, &chrono::Utc, false).len(), 4);

        assert_eq!(headers(true).last(), Some(&"Status"));
        assert_eq!(
            row_cells(&archived, &chrono::Utc, true).last().unwrap(),
            "archived"
        );
        assert_eq!(
            row_cells(&active, &chrono::Utc, true).last().unwrap(),
            "active"
        );
    }

    /// The detail view's counterpart to
    /// [`a_row_renders_every_column_from_a_real_response_body`]: driven by a
    /// real `workspace.get` envelope, not by a `WorkspaceDetail` built next to
    /// the assertion, so a field the server does not actually send shows up
    /// here as a parse failure instead of as a plausible-looking line.
    ///
    /// The body carried `cache_state` / `env_vars` / `allowed_tools` until
    /// 2026-08-09, because the server serialized its whole store type and this
    /// fixture was copied from what it really sent. It no longer sends them —
    /// they had no writer and the run pipeline never read them — so keeping
    /// them here would make a test that advertises "a real response body"
    /// the last place that lie survived. Unknown-field tolerance is a real and
    /// separate property of this projection (the server may add fields freely),
    /// and it is asserted where the type lives —
    /// `aleph_protocol::workspace`'s `a_row_ignores_fields_it_does_not_render`
    /// — not smuggled into this one's fixture.
    #[test]
    fn a_detail_renders_every_field_from_a_real_response_body() {
        let envelope: WorkspaceEnvelope = serde_json::from_value(serde_json::json!({
            "workspace": {
                "id": "crypto",
                "profile": "trading",
                "name": "Crypto Trading",
                "description": "trading notes",
                "icon": "\u{1F4B0}",
                "created_at": "2026-08-08T09:30:00Z",
                "last_active_at": "2026-08-08T11:45:00Z",
                "is_archived": false,
            }
        }))
        .expect("a real workspace.get body must parse");

        assert_eq!(
            detail_pairs(&envelope.workspace, &chrono::Utc),
            vec![
                ("ID", "crypto".to_string()),
                ("Name", "Crypto Trading".to_string()),
                ("Description", "trading notes".to_string()),
                ("Icon", "\u{1F4B0}".to_string()),
                ("Profile", "trading".to_string()),
                ("Created", "2026-08-08 09:30".to_string()),
                ("Last active", "2026-08-08 11:45".to_string()),
                ("Status", "active".to_string()),
            ]
        );
    }

    /// `workspace get` is the one view that can be about an archived
    /// workspace, so its Status line has to be able to say both things — the
    /// list's column earns its place only under `--include-archived`, this one
    /// always.
    #[test]
    fn the_detail_status_line_says_which_state_the_workspace_is_in() {
        let active = WorkspaceDetail {
            id: "crypto".to_string(),
            name: "Crypto Trading".to_string(),
            description: None,
            icon: None,
            profile: "default".to_string(),
            created_at: "2026-08-08T09:30:00Z".parse().expect("valid timestamp"),
            last_active_at: "2026-08-08T09:30:00Z".parse().expect("valid timestamp"),
            is_archived: false,
        };
        let archived = WorkspaceDetail {
            is_archived: true,
            ..active.clone()
        };

        let status = |ws| {
            detail_pairs(&ws, &chrono::Utc)
                .into_iter()
                .find(|(label, _)| *label == "Status")
                .expect("a detail view always has a Status line")
                .1
        };
        assert_eq!(status(active), "active");
        assert_eq!(status(archived), "archived");
    }

    /// An update carrying only an id is accepted by the server and changes
    /// nothing, so it would print "updated" truthfully and mean nothing. The
    /// person who typed `--nmae` cannot tell that from a change that failed to
    /// stick, which is why this is refused rather than sent.
    #[test]
    fn an_update_that_would_change_nothing_is_refused_before_the_round_trip() {
        let err = update_params("crypto", None, None, None)
            .expect_err("an empty patch must not reach the server");
        let message = err.to_string();
        assert!(message.contains("crypto"), "unexpected: {message}");
        assert!(message.contains("--name"), "unexpected: {message}");

        // Any single field is enough — this is a "not nothing" check, not an
        // "all three" one.
        for (name, description, icon) in [
            (Some("New name"), None, None),
            (None, Some("notes"), None),
            (None, None, Some("\u{1F4B0}")),
        ] {
            assert!(
                update_params("crypto", name, description, icon).is_ok(),
                "a one-field patch must be accepted"
            );
        }
    }

    /// Every field but `id` is a patch: the ones the caller omitted must be
    /// ABSENT on the wire, not sent as `null`. The server COALESCEs what it
    /// receives, so this is the difference between "leave it alone" and a
    /// client that has to be trusted to send the current value back.
    #[test]
    fn an_update_sends_only_the_fields_it_was_given() {
        let wire = serde_json::to_value(
            update_params("crypto", Some("Crypto Research"), None, None)
                .expect("a one-field patch is valid"),
        )
        .unwrap();
        assert_eq!(
            wire,
            serde_json::json!({ "id": "crypto", "name": "Crypto Research" })
        );

        let wire = serde_json::to_value(
            update_params("crypto", None, Some("notes"), Some("\u{1F4B0}"))
                .expect("a two-field patch is valid"),
        )
        .unwrap();
        assert_eq!(
            wire,
            serde_json::json!({ "id": "crypto", "description": "notes", "icon": "\u{1F4B0}" })
        );
    }

    /// `created_at` is UTC on the wire. Printed as-is it is simply the wrong
    /// time for everyone not on UTC — real-machine QA showed `09:17` for a
    /// workspace created at `17:17` local, which reads as a stale row rather
    /// than as another timezone.
    #[test]
    fn the_created_column_is_the_creation_instant_in_the_readers_zone() {
        let list: WorkspaceList = serde_json::from_value(serde_json::json!({
            "workspaces": [{
                "id": "crypto",
                "name": "Crypto Trading",
                "created_at": "2026-08-08T09:30:00Z",
            }]
        }))
        .expect("a workspace.list body must parse");

        let east8 = chrono::FixedOffset::east_opt(8 * 3600).expect("valid offset");
        assert_eq!(
            row_cells(&list.workspaces[0], &east8, false)[3],
            "2026-08-08 17:30"
        );

        let west5 = chrono::FixedOffset::west_opt(5 * 3600).expect("valid offset");
        assert_eq!(
            row_cells(&list.workspaces[0], &west5, false)[3],
            "2026-08-08 04:30"
        );
    }
}
