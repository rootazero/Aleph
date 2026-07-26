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
use crate::identity::{AgentIdentityRow, AgentLedger, LedgerRecord, NewRecord};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Records returned by `ledger` / `show` when the caller names no limit.
const DEFAULT_LIMIT: i64 = 20;
/// Hard ceiling so one call cannot pull an entire chain into the context
/// window.
const MAX_LIMIT: i64 = 200;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentIdentityArgs {
    /// One of: "list", "show", "keygen", "rotate", "revoke", "ledger", "verify".
    pub action: String,
    /// Agent id. Required for "show", "keygen", "rotate" and "revoke";
    /// optional for "ledger" and "verify" (omit to span every agent).
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
        // How far this agent's chain has advanced. Compared against the stored
        // rows by `verify` — a mismatch is the truncation signal.
        "records": row.head_seq,
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

Rotation and revocation are themselves appended to the affected agent's chain, so key history cannot be quietly rewritten by editing the database. A delegated sub-agent holds its own key and signs its own work; it is a separate agent here, not a line on its parent's chain.

What a clean verify does and does not prove: it proves no stored record was edited, reordered, deleted or forged without the agent's private key. It does not defend against someone who controls the whole machine — key vault and database sit on the same disk. Export the public fingerprints and check an exported chain elsewhere if you need a proof that does not trust this host.

Arguments are never stored; each record carries a fingerprint of them, plus a secret-redacted one-line summary."#;

    type Args = AgentIdentityArgs;
    type Output = Value;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let ledger = Self::ledger()?;
        let keys = ledger.keys();

        match args.action.as_str() {
            "list" => {
                let rows = keys.list().map_err(|e| AlephError::tool(e.to_string()))?;
                Ok(json!({
                    "action": "list",
                    "agents": rows.iter().map(identity_json).collect::<Vec<_>>(),
                    "failed_appends": ledger.lost(),
                }))
            }

            "show" => {
                let agent = Self::agent_of(&args, "show")?;
                let identity = keys
                    .identity(&agent)
                    .map_err(|e| AlephError::tool(e.to_string()))?
                    .ok_or_else(|| {
                        AlephError::tool(format!("agent '{agent}' has no identity yet"))
                    })?;
                let all_keys = keys
                    .keys_of(&agent)
                    .map_err(|e| AlephError::tool(e.to_string()))?;
                let recent = ledger
                    .recent(Some(&agent), Self::limit_of(&args))
                    .map_err(|e| AlephError::tool(e.to_string()))?;
                Ok(json!({
                    "action": "show",
                    "identity": identity_json(&identity),
                    "keys": all_keys.iter().map(|k| json!({
                        "fingerprint": k.fingerprint,
                        "created_at": at(k.created_at),
                        "retired_at": k.retired_at.map(at),
                        "active": k.fingerprint == identity.active_fingerprint,
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

            "rotate" => {
                let agent = Self::agent_of(&args, "rotate")?;
                let previous = keys
                    .identity(&agent)
                    .map_err(|e| AlephError::tool(e.to_string()))?
                    .map(|r| r.active_fingerprint);
                let row = keys
                    .rotate(&agent)
                    .map_err(|e| AlephError::tool(e.to_string()))?;
                // Into the SUBJECT's chain, not the operator's: `retired_at` is
                // an ordinary mutable column, so without this the fact that a
                // key was ever replaced can be erased and the chain still
                // verifies clean. The operator's own chain separately carries
                // the `agent_identity` tool call that caused it.
                crate::identity::record_action(NewRecord::identity_rotated(
                    &agent,
                    &row.active_fingerprint,
                    previous.as_deref(),
                ))
                .await;
                Ok(json!({
                    "action": "rotate",
                    "identity": identity_json(&row),
                    "previous_fingerprint": previous,
                }))
            }

            "revoke" => {
                let agent = Self::agent_of(&args, "revoke")?;
                // Read the key first: it is the one that will sign the
                // revocation, and after this call it is retired.
                let fingerprint = keys
                    .identity(&agent)
                    .map_err(|e| AlephError::tool(e.to_string()))?
                    .map(|r| r.active_fingerprint);
                let revoked = keys
                    .revoke(&agent)
                    .map_err(|e| AlephError::tool(e.to_string()))?;
                // Only when something was actually revoked — recording against
                // an agent that has no identity would mint a key just to say it
                // was taken away. The revocation is signed by the key being
                // revoked (see `AgentKeystore::signing_identity`), which is the
                // whole reason that method tolerates a revoked agent.
                if let (true, Some(fp)) = (revoked, fingerprint.as_deref()) {
                    crate::identity::record_action(NewRecord::identity_revoked(&agent, fp)).await;
                }
                Ok(json!({
                    "action": "revoke",
                    "agent": agent,
                    // false = there was no live identity to revoke.
                    "revoked": revoked,
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

            other => Err(AlephError::tool(format!(
                "action must be one of list, show, keygen, rotate, revoke, ledger, verify — \
                 got '{other}'"
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
