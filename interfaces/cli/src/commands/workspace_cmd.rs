//! Workspace management commands
//!
//! The request/response shapes are `aleph_protocol::workspace::*` — the same
//! types the server deserializes. They are not re-declared here on purpose;
//! see that module for what happened the last time they were.

use chrono::TimeZone;
use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliResult};
use aleph_protocol::workspace::{
    WorkspaceCreateParams, WorkspaceList, WorkspaceListParams, WorkspaceRef, WorkspaceRow,
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
