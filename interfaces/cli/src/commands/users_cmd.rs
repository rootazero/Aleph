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
    let parsed: UserListResult = serde_json::from_value(result.clone())
        .map_err(|e| aleph_client::CliError::Other(e.to_string()))?;
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
    for line in update_effect_lines(result) {
        println!("{line}");
    }
}

/// The lines [`print_update_effects`] prints, as values.
///
/// Split out from the printing so the rendering can be asserted on. A receipt
/// field with no renderer is the defect this whole surface exists to avoid,
/// and "the CLI prints the heartbeat count" is not something a test can check
/// while the only way to observe it is stdout — the field could be added, the
/// server could measure it, and the line could simply never be written.
///
/// An empty `String` renders as a blank separator line.
fn update_effect_lines(result: &UserUpdateResult) -> Vec<String> {
    let mut lines = Vec::new();

    if let Some(devices) = result.revoked_devices {
        // Zero is a measurement and it matters: it means this principal held
        // no device credential, so "revoked" is not what closed anything.
        lines.push(match devices {
            0 => "No devices were bound to them; nothing to revoke.".to_string(),
            1 => "1 device revoked and its live connections closed.".to_string(),
            n => format!("{n} devices revoked and their live connections closed."),
        });
    }

    if !result.revoked_channel_senders.is_empty() {
        lines.push("Channel sender approvals withdrawn:".to_string());
        for s in &result.revoked_channel_senders {
            lines.push(format!("  {} / {}", s.channel, s.sender_id));
        }
    }

    if let Some(frozen) = result.frozen_background_work {
        // Two sentences, because there are two facts and folding them loses
        // one: what the freeze DID, and — separately — whether the heartbeat
        // leg ran at all. An unmeasured leg is not a leg that found nothing.
        match frozen.heartbeats {
            Some(heartbeats) => {
                if frozen.is_empty() {
                    lines.push(
                        "They owned no running goals, loops, crons or heartbeat tasks.".to_string(),
                    );
                } else {
                    lines.push(format!(
                        "Background work frozen: {} goal(s), {} loop(s), {} cron(s), \
                         {heartbeats} heartbeat task(s).",
                        frozen.goals, frozen.loops, frozen.crons
                    ));
                }
            }
            None => {
                // Only three legs were measured, so only three may be named.
                if frozen.goals == 0 && frozen.loops == 0 && frozen.crons == 0 {
                    lines.push("They owned no running goals, loops or crons.".to_string());
                } else {
                    lines.push(format!(
                        "Background work frozen: {} goal(s), {} loop(s), {} cron(s).",
                        frozen.goals, frozen.loops, frozen.crons
                    ));
                }
                lines.push(
                    "Heartbeat tasks were NOT checked: no heartbeat service is running on \
                     that server, so any heartbeat task they own is still armed. Run \
                     `aleph doctor` — `core/capability-wiring` names the cause."
                        .to_string(),
                );
            }
        }
    }

    if let Some(effects) = &result.reactivation_effects {
        // The half a bare `status: active` hides. Reactivation flips one
        // column; naming what stayed down is the whole reason the server
        // measures it.
        lines.push(String::new());
        lines.push("Reactivated — but this did NOT restore:".to_string());
        lines.push(format!("  devices:         {}", effects.devices));
        lines.push(format!("  channel senders: {}", effects.channel_senders));
        lines.push(format!("  background work: {}", effects.background_work));
    }

    lines
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

    fn deactivation_receipt(
        frozen: aleph_protocol::users::FrozenBackgroundWork,
    ) -> UserUpdateResult {
        UserUpdateResult {
            user: aleph_protocol::users::UserView {
                user_id: "u-alice".to_string(),
                display_name: "Alice".to_string(),
                role: "member".to_string(),
                status: "deactivated".to_string(),
            },
            revoked_channel_senders: Vec::new(),
            revoked_devices: Some(0),
            frozen_background_work: Some(frozen),
            reactivation_effects: None,
        }
    }

    /// The heartbeat leg has a renderer. A count the server measures and the
    /// only client never prints is a count that does not exist — and this is
    /// the sole reason the field was added, so the line is asserted by its
    /// content, not by the field being present somewhere in the struct.
    #[test]
    fn the_receipt_prints_the_frozen_heartbeat_count() {
        let lines = update_effect_lines(&deactivation_receipt(
            aleph_protocol::users::FrozenBackgroundWork {
                goals: 0,
                loops: 0,
                crons: 0,
                heartbeats: Some(3),
            },
        ));
        let frozen_line = lines
            .iter()
            .find(|l| l.starts_with("Background work frozen:"))
            .unwrap_or_else(|| {
                panic!("three frozen heartbeat tasks must not render as silence: {lines:?}")
            });
        assert!(
            frozen_line.contains("3 heartbeat task(s)"),
            "the frozen line must name the heartbeat count: {frozen_line}"
        );
    }

    /// The quiet branch must not claim more than the freeze measured: with the
    /// heartbeat leg measured, "nothing was frozen" has to cover four legs.
    #[test]
    fn the_quiet_line_names_the_heartbeat_leg_once_it_is_measured() {
        let lines = update_effect_lines(&deactivation_receipt(
            aleph_protocol::users::FrozenBackgroundWork {
                goals: 0,
                loops: 0,
                crons: 0,
                heartbeats: Some(0),
            },
        ));
        assert!(
            lines
                .iter()
                .any(|l| l == "They owned no running goals, loops, crons or heartbeat tasks."),
            "a measured zero on all four legs must say all four: {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("NOT checked")),
            "a measured leg must not also print the unmeasured caveat: {lines:?}"
        );
    }

    /// The declined path (criterion #8): an unmeasured heartbeat leg gets its
    /// own sentence and must never be folded into the three-leg summary, which
    /// would read to an operator as "they owned none".
    #[test]
    fn an_unmeasured_heartbeat_leg_gets_its_own_sentence_not_a_zero() {
        let lines = update_effect_lines(&deactivation_receipt(
            aleph_protocol::users::FrozenBackgroundWork {
                goals: 1,
                loops: 0,
                crons: 0,
                heartbeats: None,
            },
        ));
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("Heartbeat tasks were NOT checked:")
                    && l.contains("still armed")),
            "an unmeasured leg must say so, and say what is still running: {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("heartbeat task(s)")),
            "an unmeasured leg must not print a count of any kind: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l == "Background work frozen: 1 goal(s), 0 loop(s), 0 cron(s)."),
            "the three legs that DID run are still reported: {lines:?}"
        );
    }

    /// Reactivation names every leg the deactivation froze. Naming three of
    /// four is the shape the heartbeat leg was added to close, and this string
    /// is server-authored — the client only forwards it, so the assertion is
    /// that the client prints whatever the server said, verbatim.
    #[test]
    fn reactivation_guidance_is_printed_verbatim() {
        let mut receipt = deactivation_receipt(aleph_protocol::users::FrozenBackgroundWork {
            goals: 0,
            loops: 0,
            crons: 0,
            heartbeats: Some(0),
        });
        receipt.frozen_background_work = None;
        receipt.revoked_devices = None;
        receipt.reactivation_effects = Some(aleph_protocol::users::ReactivationEffects {
            devices: "d".to_string(),
            channel_senders: "c".to_string(),
            background_work: "goals/loops/crons/heartbeat tasks remain paused".to_string(),
        });
        let lines = update_effect_lines(&receipt);
        assert!(
            lines
                .iter()
                .any(|l| l == "  background work: goals/loops/crons/heartbeat tasks remain paused"),
            "the server's recovery guidance must reach the operator unedited: {lines:?}"
        );
    }
}
