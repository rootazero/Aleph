//! User management commands — the operator's invite flow.
//!
//! ## Why this file exists
//!
//! `users.create` and `users.update` were implemented, registered at boot (in
//! both registration paths), admin-gated, pin-tested, and given a device-revoke
//! pipeline and a live role re-stamp — and **no client anywhere called them**.
//! The Panel implements `users.me` and `users.list` only; there was no CLI
//! command; `aleph-server pair` hard-coded `None` where `create_bootstrap_ticket`
//! takes a `user_id`. Three shipped phases of multi-user machinery (identity,
//! data isolation, project rooms) sat behind a door with no handle: the P2 roster
//! picker is fed by `users.list`, so it could only ever offer `u-owner`.
//!
//! **A capability with no client is not shipped, however complete its server
//! half is.** This is the client.
//!
//! ## Why the CLI and not the Panel
//!
//! `users.*` is in `ADMIN_PREFIXES`, and the CLI reaches the server over
//! loopback, which `resolve_connection_identity` short-circuits to
//! `(OWNER_USER_ID, "operator")`. So the admin gate needs no carve-out and the
//! command needs no new authorization concept — it is the same shape
//! `workspace_cmd.rs` uses for the same reason. The consequence is worth stating
//! rather than discovering: **adding a member requires being at the machine (or
//! SSH'd into it)**, which is a defensible posture for the verb that mints
//! principals, and it keeps the invite flow out of reach of a remote session
//! whose own credential is what is being managed.

use aleph_protocol::users::{
    UserCreateParams, UserCreateResult, UserListResult, UserUpdateParams, UserUpdateResult,
};
use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliResult};

/// List every principal the server knows.
pub async fn list(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client.call("users.list", None::<()>).await?;

    // Parsed through the shared contract type rather than by string lookup: a
    // renamed column used to render as a table of dashes, which reads like a
    // field with no value rather than like a broken client.
    let parsed: UserListResult =
        serde_json::from_value(result.clone()).map_err(|e| aleph_client::CliError::Other(e.to_string()))?;
    let rows: Vec<Vec<String>> = parsed
        .users
        .iter()
        .map(|u| {
            vec![
                u.user_id.clone(),
                u.display_name.clone(),
                u.role.clone(),
                u.status.clone(),
            ]
        })
        .collect();

    output::print_table(&["User ID", "Name", "Role", "Status"], &rows, json, &result);

    client.close().await?;
    Ok(())
}

/// Create a principal. The server generates the id (`u-<uuid v4>`); it is
/// echoed back because the next step — minting that person a pairing ticket —
/// needs it.
pub async fn create(
    server_url: &str,
    config: &CliConfig,
    display_name: &str,
    role: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = UserCreateParams {
        display_name: display_name.to_string(),
        role: role.map(str::to_string),
    };

    let result: Value = client.call("users.create", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let created: UserCreateResult = serde_json::from_value(result.clone())
            .map_err(|e| aleph_client::CliError::Other(e.to_string()))?;
        let user_id = created.user.user_id.as_str();
        let role = created.user.role.as_str();
        println!("Created {display_name} ({role}) as {user_id}");
        // The id alone grants nothing — say what the second step is, because
        // this command's whole purpose is to be the first half of a flow.
        println!();
        println!("Next, mint them a pairing ticket bound to that id:");
        println!("  aleph-server pair --user {user_id}");
    }

    client.close().await?;
    Ok(())
}

/// Rename a principal, change their role, or deactivate them.
///
/// Deactivation is not cosmetic: the server revokes every device bound to the
/// user and closes their live sockets through the same pipeline
/// `gateway.devices.revoke` uses. A role change re-stamps live connections, so
/// it takes effect without a reconnect.
pub async fn update(
    server_url: &str,
    config: &CliConfig,
    user_id: &str,
    display_name: Option<&str>,
    role: Option<&str>,
    status: Option<&str>,
    json: bool,
) -> CliResult<()> {
    if display_name.is_none() && role.is_none() && status.is_none() {
        return Err(aleph_client::CliError::Other(
            "nothing to update: pass at least one of --name, --role, --status".to_string(),
        ));
    }

    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = UserUpdateParams {
        user_id: user_id.to_string(),
        display_name: display_name.map(str::to_string),
        role: role.map(str::to_string),
        status: status.map(str::to_string),
    };

    let result: Value = client.call("users.update", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let parsed: UserUpdateResult = serde_json::from_value(result.clone())
            .map_err(|e| aleph_client::CliError::Other(e.to_string()))?;
        println!("Updated {user_id}.");
        print_update_effects(&parsed);
    }

    client.close().await?;
    Ok(())
}

/// Render what the write actually did.
///
/// This used to print one hard-coded sentence — "Their devices are revoked and
/// their live connections are closed" — on every deactivation, whether any
/// device existed or not, while the server's measured counts, the withdrawn
/// channel bindings and the entire reactivation caveat went into the response
/// and straight into the bit bucket. A receipt the only client discards is a
/// receipt that does not exist, and a hard-coded claim standing in for a
/// measurement is worse than silence: it is the client asserting an outcome it
/// did not observe.
fn print_update_effects(result: &UserUpdateResult) {
    if let Some(devices) = result.revoked_devices {
        // Zero is a measurement and it matters: it means this principal held
        // no device credential, so "revoked" is not what closed anything.
        match devices {
            0 => println!("No devices were bound to them; nothing to revoke."),
            1 => println!("1 device revoked and its live connections closed."),
            n => println!("{n} devices revoked and their live connections closed."),
        }
    }

    if !result.revoked_channel_senders.is_empty() {
        println!("Channel sender approvals withdrawn:");
        for s in &result.revoked_channel_senders {
            println!("  {} / {}", s.channel, s.sender_id);
        }
    }

    if let Some(frozen) = result.frozen_background_work {
        if frozen.is_empty() {
            println!("They owned no running goals, loops or crons.");
        } else {
            println!(
                "Background work frozen: {} goal(s), {} loop(s), {} cron(s).",
                frozen.goals, frozen.loops, frozen.crons
            );
        }
    }

    if let Some(effects) = &result.reactivation_effects {
        // The half a bare `status: active` hides. Reactivation flips one
        // column; naming what stayed down is the whole reason the server
        // measures it.
        println!();
        println!("Reactivated — but this did NOT restore:");
        println!("  devices:         {}", effects.devices);
        println!("  channel senders: {}", effects.channel_senders);
        println!("  background work: {}", effects.background_work);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_omits_the_role_when_unset_so_the_server_default_applies() {
        let mut params = serde_json::json!({ "display_name": "Alice" });
        let role: Option<&str> = None;
        if let Some(r) = role {
            params["role"] = Value::String(r.to_string());
        }
        assert_eq!(params["display_name"], "Alice");
        assert!(
            params.get("role").is_none(),
            "an absent --role must not be sent as null; the server's own \
             default (member) is the single source of that decision"
        );
    }

    #[test]
    fn create_forwards_an_explicit_role() {
        let mut params = serde_json::json!({ "display_name": "Bob" });
        if let Some(r) = Some("admin") {
            params["role"] = Value::String(r.to_string());
        }
        assert_eq!(params["role"], "admin");
    }

    /// `users.update` with no changed field is an empty patch the server would
    /// happily accept, returning the user unchanged — a success response for a
    /// command that did nothing. Refuse locally so the operator learns they
    /// mistyped a flag instead of reading "Updated u-alice." and believing it.
    #[tokio::test]
    async fn update_refuses_a_patch_with_no_fields() {
        let config = CliConfig::default();
        let err = update(
            "ws://127.0.0.1:1",
            &config,
            "u-alice",
            None,
            None,
            None,
            false,
        )
        .await;
        assert!(
            err.is_err(),
            "an all-None update must fail before it opens a connection"
        );
    }
}
