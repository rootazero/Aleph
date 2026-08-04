//! `agent_identity`: the read/verify/manage surface for per-agent signing keys
//! and the signed operation ledger.
//!
//! R8 ("everything is a tool"): without this, the ledger would be a table
//! nobody can read — which is precisely how the previous approval-audit store
//! ended up deleted (`aleph-server audit` queried three tables whose only
//! writers were test helpers; an operator ran it, saw zeros, and concluded
//! nothing had happened). A record surface ships with its reader or it does not
//! ship.
//!
//! Redline posture: pure I/O translation (R4) over
//! [`crate::identity`] — the tool chooses nothing, computes no digest, and
//! reaches no verdict of its own (R7). Operator-gated (`OPERATOR_TOOLS`):
//! rotating or revoking an agent's key is not a chat-tier capability.
//!
//! Declared **non-idempotent** on purpose: `rotate` and `revoke` mutate, and
//! the tier reads a tool's declared metadata, not its arguments. Under the
//! `Ask` tier that means `verify` prompts too. That is the fail-closed side of
//! the trade, and it is the right one — the alternative is a second
//! argument-level tier rule, and the codebase deliberately has exactly one.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{AlephError, Result};
use crate::identity::{AgentIdentityRow, AgentLedger, LedgerRecord};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Where `export` writes. A fixed directory with a derived filename, never a
/// caller-supplied path: the tool gains no filesystem reach it did not already
/// have, and there is no traversal to get wrong.
const EXPORT_DIR: &str = "exports";

/// Records returned by `ledger` / `show` when the caller names no limit.
const DEFAULT_LIMIT: i64 = 20;
/// Hard ceiling so one call cannot pull an entire chain into the context
/// window.
const MAX_LIMIT: i64 = 200;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentIdentityArgs {
    /// One of: "list", "show", "keygen", "rotate", "revoke", "ledger",
    /// "verify", "export".
    pub action: String,
    /// Agent id. Required for "show", "keygen", "rotate", "revoke" and
    /// "export"; optional for "ledger" and "verify" (omit to span every agent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Max records to return for "ledger" / "show". Default 20, capped at 200.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

#[derive(Clone, Default)]
pub struct AgentIdentityTool;

impl AgentIdentityTool {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// The installed ledger, or a message saying plainly that identity
    /// recording is not running — never an empty result that reads like
    /// "nothing has happened".
    fn ledger() -> Result<Arc<AgentLedger>> {
        crate::identity::global().ok_or_else(|| {
            AlephError::tool(
                "the agent identity ledger is not installed in this process, so there is \
                 nothing to read. This is not an empty ledger — no records are being \
                 written at all. It is installed by `aleph-server start`.",
            )
        })
    }

    fn agent_of(args: &AgentIdentityArgs, action: &str) -> Result<String> {
        args.agent
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_string)
            .ok_or_else(|| AlephError::tool(format!("{action}: `agent` is required")))
    }

    fn limit_of(args: &AgentIdentityArgs) -> i64 {
        args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
    }

    /// A filename component that cannot escape [`EXPORT_DIR`]. Agent ids are
    /// well-behaved in practice; this makes that a property of the code rather
    /// than of the practice.
    fn file_slug(agent: &str) -> String {
        agent
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    }
}

/// Epoch millis rendered for a human, alongside the raw value.
fn at(ms: i64) -> Value {
    match chrono::DateTime::from_timestamp_millis(ms) {
        Some(dt) => json!(dt.to_rfc3339()),
        None => json!(ms),
    }
}

fn identity_json(row: &AgentIdentityRow) -> Value {
    json!({
        "agent": row.agent_id,
        "fingerprint": row.active_fingerprint,
        "created_at": at(row.created_at),
        "revoked_at": row.revoked_at.map(at),
        // How far this agent's chain has advanced, per the ANCHOR — not a count
        // of rows on disk. `verify` compares the two, and a mismatch is the
        // truncation signal, so calling this "records" (as it was) invited the
        // one reading that erases the distinction the anchor exists to draw.
        "chain_head": row.head_seq,
    })
}

fn record_json(r: &LedgerRecord) -> Value {
    json!({
        "agent": r.agent_id,
        "seq": r.seq,
        "at": at(r.at_ms),
        "action": r.action,
        "target": r.target,
        "outcome": r.outcome,
        "detail": r.detail,
        "args_fingerprint": r.args_fp,
        "signer": r.signer_fp,
    })
}

#[async_trait]
impl AlephTool for AgentIdentityTool {
    const NAME: &'static str = "agent_identity";
    const DESCRIPTION: &'static str = r#"Per-agent signing keys and the tamper-evident record of what each agent did.

Every agent holds its own Ed25519 keypair. Every mutating tool call, every refusal and every approval decision is appended to that agent's own hash-chained, signed ledger, so "who did this" has an answer that does not depend on trusting the log.

- {"action": "list"} — every agent identity: key fingerprint, when it was minted, whether it is revoked, and how many records its chain holds.
- {"action": "show", "agent": "main"} — one agent in full: its active key, every key it has ever held (retired keys are kept so their records stay verifiable), and its most recent records.
- {"action": "ledger", "agent": "main", "limit": 50} — recent records, newest first. Omit `agent` to span every agent.
- {"action": "verify", "agent": "main"} — recompute the chain and check every signature. Omit `agent` to verify all. Reports ok=true, or every fault found: an edited row, a broken link, a bad signature, a deleted prefix, a sequence gap, or a truncated tail.
- {"action": "keygen", "agent": "main"} — mint the key if the agent does not have one yet (agents get one automatically on first recorded action).
- {"action": "rotate", "agent": "main"} — replace the signing key. History signed by the old key stays verifiable; the chain is NOT reset.
- {"action": "revoke", "agent": "main"} — stop the agent signing. Its chain stays readable and verifiable. `rotate` brings a revoked agent back.
- {"action": "export", "agent": "main"} — write the agent's whole chain, its public keys and its anchor to one self-contained JSON file, and report the path. Contains no private key and no tool arguments. Hand it to an auditor: `aleph-server identity verify --input <path> --pin <root_fingerprint> --expect-head <expect_head>` checks it on a machine with no Aleph, no database and no daemon.

Rotation and revocation are themselves appended to the affected agent's chain, so key history cannot be quietly rewritten by editing the database. Both wait for that record to be written and report a failure rather than claiming a success the chain does not know about. A delegated sub-agent holds its own key and signs its own work; it is a separate agent here, not a line on its parent's chain.

What a clean verify does and does not prove: it proves no stored record was edited, reordered, deleted, re-signed under an undeclared key, or forged without the agent's private key. It does not defend against someone who controls the whole machine — key vault and database sit on the same disk. That is what `export` plus pinned values is for, and there are two, each covering what the other cannot. `root_fingerprint` pins the lineage: pin it once, off this host, and no later export can present a different chain as this agent's. `expect_head` pins where the chain had got to: the next export must extend it, which is the ONLY way a truncated tail can be caught — the anchor travels inside the file, so whoever produced the file can edit it as freely as the rows. Report both values to the user when you export, and tell them to keep them somewhere other than this machine. With neither, an export proves internal consistency only.

Arguments are never stored; each record carries a fingerprint of them, plus a secret-redacted one-line summary."#;

    type Args = AgentIdentityArgs;
    type Output = Value;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let ledger = Self::ledger()?;
        let keys = ledger.keys();

        match args.action.as_str() {
            "list" => {
                let rows = keys.list().map_err(|e| AlephError::tool(e.to_string()))?;
                // Chains whose identity row is gone are absent from `rows` by
                // construction — that is what makes deleting one row an attack.
                // Naming them here keeps the listing from being quietly
                // narrower than the evidence.
                let orphans = ledger
                    .orphan_chains()
                    .map_err(|e| AlephError::tool(e.to_string()))?;
                Ok(json!({
                    "action": "list",
                    "agents": rows.iter().map(identity_json).collect::<Vec<_>>(),
                    "orphan_chains": orphans,
                    "failed_appends": ledger.lost(),
                }))
            }

            "show" => {
                let agent = Self::agent_of(&args, "show")?;
                let identity = keys
                    .identity(&agent)
                    .map_err(|e| AlephError::tool(e.to_string()))?;
                let all_keys = keys
                    .keys_of(&agent)
                    .map_err(|e| AlephError::tool(e.to_string()))?;
                let recent = ledger
                    .recent(Some(&agent), Self::limit_of(&args))
                    .map_err(|e| AlephError::tool(e.to_string()))?;
                // An agent with records but no identity row is shown, not
                // refused. Refusing (what this did) meant the one tamper that
                // removes an agent from every listing also made the surface
                // that could reveal it answer "no identity yet" — reading, to
                // an operator, as "nothing to see".
                if identity.is_none() && recent.is_empty() {
                    return Err(AlephError::tool(format!(
                        "agent '{agent}' has no identity and no records"
                    )));
                }
                let active = identity.as_ref().map(|i| i.active_fingerprint.as_str());
                Ok(json!({
                    "action": "show",
                    "identity": identity.as_ref().map(identity_json),
                    "identity_row_missing": identity.is_none(),
                    "keys": all_keys.iter().map(|k| json!({
                        "fingerprint": k.fingerprint,
                        "created_at": at(k.created_at),
                        "retired_at": k.retired_at.map(at),
                        "active": Some(k.fingerprint.as_str()) == active,
                    })).collect::<Vec<_>>(),
                    "recent": recent.iter().map(record_json).collect::<Vec<_>>(),
                }))
            }

            "keygen" => {
                let agent = Self::agent_of(&args, "keygen")?;
                let existing = keys
                    .identity(&agent)
                    .map_err(|e| AlephError::tool(e.to_string()))?;
                let row = keys
                    .ensure(&agent)
                    .map_err(|e| AlephError::tool(e.to_string()))?;
                Ok(json!({
                    "action": "keygen",
                    "identity": identity_json(&row),
                    // false = this agent already had a key and it was returned
                    // unchanged (the call is idempotent).
                    "created": existing.is_none(),
                }))
            }

            // Both lifecycle actions go through the ledger writer and are
            // AWAITED. They used to mutate the keystore here and then enqueue
            // the chain record fire-and-forget, which made the tool's `ok` a
            // claim about something that had not happened yet: a declaration
            // lost on the way to disk leaves the incoming key active and
            // undeclared, and every record it goes on to sign faults as
            // `UndeclaredSigner` forever. The writer now owns both halves,
            // ordered so that a failure changes nothing (see
            // `AgentLedger::perform_rotate`), and a failure is reported.
            "rotate" => {
                let agent = Self::agent_of(&args, "rotate")?;
                let rotation = crate::identity::rotate_identity(&agent)
                    .await
                    .map_err(|e| AlephError::tool(e.to_string()))?;
                Ok(json!({
                    "action": "rotate",
                    "identity": identity_json(&rotation.identity),
                    "previous_fingerprint": rotation.previous_fingerprint,
                    // The rotation is on the subject's own chain, not the
                    // operator's: `retired_at` is an ordinary mutable column, so
                    // without an in-chain statement the fact that a key was ever
                    // replaced can be erased and the chain still verifies clean.
                    // The operator's chain separately carries this tool call.
                    "declared_in_chain": true,
                }))
            }

            "revoke" => {
                let agent = Self::agent_of(&args, "revoke")?;
                let retired = crate::identity::revoke_identity(&agent)
                    .await
                    .map_err(|e| AlephError::tool(e.to_string()))?;
                Ok(json!({
                    "action": "revoke",
                    "agent": agent,
                    // false = there was no live identity to revoke.
                    "revoked": retired.is_some(),
                    "retired_fingerprint": retired,
                }))
            }

            "ledger" => {
                let agent = args
                    .agent
                    .as_deref()
                    .map(str::trim)
                    .filter(|a| !a.is_empty());
                let records = ledger
                    .recent(agent, Self::limit_of(&args))
                    .map_err(|e| AlephError::tool(e.to_string()))?;
                Ok(json!({
                    "action": "ledger",
                    "agent": agent,
                    "records": records.iter().map(record_json).collect::<Vec<_>>(),
                    "failed_appends": ledger.lost(),
                }))
            }

            "verify" => {
                let reports = match args
                    .agent
                    .as_deref()
                    .map(str::trim)
                    .filter(|a| !a.is_empty())
                {
                    Some(agent) => vec![ledger
                        .verify(agent)
                        .map_err(|e| AlephError::tool(e.to_string()))?],
                    None => ledger
                        .verify_all()
                        .map_err(|e| AlephError::tool(e.to_string()))?,
                };
                let ok = reports.iter().all(|r| r.ok);
                Ok(json!({
                    "action": "verify",
                    "ok": ok,
                    "reports": reports,
                    // Non-zero means records were lost before they were ever
                    // written, which no chain check can see. Reported next to
                    // the verdict so a clean `ok` is never read as "complete".
                    "failed_appends": ledger.lost(),
                }))
            }

            "export" => {
                let agent = Self::agent_of(&args, "export")?;
                let doc = crate::identity::export_chain(&ledger, &agent)
                    .map_err(|e| AlephError::tool(e.to_string()))?;
                // Verified here, with the document's own keys, before it is
                // handed over: an export that does not verify at the moment it
                // is produced is the single most useful thing to learn early.
                let report =
                    crate::identity::verify_export(&doc, &crate::identity::ExportPins::default())
                        .map_err(|e| AlephError::tool(e.to_string()))?;

                // Written to a file rather than returned inline: a chain is
                // unbounded, and the document exists to be handed to someone,
                // not to be read by the model. Returning it would spend the
                // context window on bytes nobody in this conversation reads.
                let dir = crate::utils::paths::get_data_dir()
                    .map_err(|e| AlephError::tool(format!("cannot locate the data dir: {e}")))?
                    .join(EXPORT_DIR);
                std::fs::create_dir_all(&dir).map_err(|e| {
                    AlephError::tool(format!("cannot create {}: {e}", dir.display()))
                })?;
                let path = dir.join(format!(
                    "agent-chain-{}-{}.json",
                    Self::file_slug(&agent),
                    report.last_seq
                ));
                let body = serde_json::to_string_pretty(&doc)
                    .map_err(|e| AlephError::tool(e.to_string()))?;
                std::fs::write(&path, body).map_err(|e| {
                    AlephError::tool(format!("cannot write {}: {e}", path.display()))
                })?;

                // The head, in the exact form `--expect-head` accepts. Emitted
                // ready to paste because the alternative — telling an auditor to
                // note two numbers and compare them next time — is how the one
                // defence against a truncated tail stays theoretical.
                let expect_head = report
                    .last_hash
                    .as_ref()
                    .map(|h| format!("{}:{h}", report.last_seq));
                Ok(json!({
                    "action": "export",
                    "agent": agent,
                    "path": path.display().to_string(),
                    "records": report.records,
                    "last_seq": report.last_seq,
                    "verifies_now": report.ok,
                    "faults": report.faults,
                    // The two values to pin off-box. Everything the export can
                    // prove to a third party rests on these having been taken
                    // from somewhere other than this machine.
                    "root_fingerprint": report.root_fingerprint,
                    "expect_head": expect_head,
                    "revoked_at": report.revoked_at.map(at),
                    "revocation_disagrees_with_chain": report.revocation_disagrees(),
                    "failed_appends": report.failed_appends,
                    "verify_with": "aleph-server identity verify --input <path> \
                                    --pin <root_fingerprint> --expect-head <expect_head>",
                }))
            }

            other => Err(AlephError::tool(format!(
                "action must be one of list, show, keygen, rotate, revoke, ledger, verify, \
                 export — got '{other}'"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(action: &str) -> AgentIdentityArgs {
        AgentIdentityArgs {
            action: action.to_string(),
            agent: None,
            limit: None,
        }
    }

    #[tokio::test]
    async fn without_an_installed_ledger_it_says_so_instead_of_returning_empty() {
        // The failure mode this whole subsystem exists to avoid: a reader that
        // returns zeros when the recorder is not running.
        let err = AgentIdentityTool::new()
            .call(args("list"))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("not installed"),
            "expected an explicit not-running message, got: {err}"
        );
    }

    #[test]
    fn the_catalog_ships_the_tools_own_description() {
        // `agent_init` builds the LLM tool list from `BUILTIN_TOOL_DEFINITIONS`
        // and only *appends* registry tools whose name the catalog does not
        // already carry — so a literal written into that entry SHADOWS this
        // const outright, and nothing reports it. The literal that used to sit
        // there listed seven actions and did not mention `export` at all: the
        // whole pin-the-root-fingerprint contract was written, tested against
        // the const, and shipped to nobody.
        //
        // Asserted on the catalog side, because that is the side that reaches
        // the model. A test that read this const would stay green through
        // exactly the regression it is here to catch.
        let entry = crate::executor::BUILTIN_TOOL_DEFINITIONS
            .iter()
            .find(|d| d.name == AgentIdentityTool::NAME)
            .expect("agent_identity must be registered");
        assert_eq!(
            entry.description,
            AgentIdentityTool::DESCRIPTION,
            "the catalog entry must point at the const, not restate it"
        );
        for action in [
            "list", "show", "keygen", "rotate", "revoke", "ledger", "verify", "export",
        ] {
            assert!(
                entry
                    .description
                    .contains(&format!("\"action\": \"{action}\"")),
                "the model is never told about action `{action}`"
            );
        }
    }

    #[test]
    fn limit_is_defaulted_and_capped() {
        let mut a = args("ledger");
        assert_eq!(AgentIdentityTool::limit_of(&a), DEFAULT_LIMIT);
        a.limit = Some(100_000);
        assert_eq!(AgentIdentityTool::limit_of(&a), MAX_LIMIT);
        a.limit = Some(0);
        assert_eq!(AgentIdentityTool::limit_of(&a), 1);
    }

    #[test]
    fn agent_is_required_where_it_matters_and_blank_does_not_count() {
        let mut a = args("rotate");
        assert!(AgentIdentityTool::agent_of(&a, "rotate").is_err());
        a.agent = Some("   ".into());
        assert!(AgentIdentityTool::agent_of(&a, "rotate").is_err());
        a.agent = Some("  main ".into());
        assert_eq!(AgentIdentityTool::agent_of(&a, "rotate").unwrap(), "main");
    }
}
