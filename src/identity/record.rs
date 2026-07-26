//! Ledger record types.
//!
//! [`NewRecord`] is deliberately **not** `Serialize`/`Deserialize`: it is an
//! in-process input consumed by the single ledger writer, and refusing
//! deserialization means there is no path by which a client-supplied blob
//! becomes a ledger entry claiming an `agent_id`. (Borrowed from buzz's
//! `NewAuditEntry` provenance fence.)

use std::fmt;
use std::str::FromStr;

use serde::Serialize;

/// What an agent did, as recorded in its chain.
///
/// Deliberately small and closed. Every variant below has a real producer —
/// a declared-but-never-constructed variant is how an audit surface starts
/// lying about coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerAction {
    /// A non-idempotent (mutating) tool call ran to completion.
    ToolCall,
    /// A tool call was refused by a gate before it ran.
    ToolDenied,
    /// A human (or operator) approved a gated call.
    ApprovalGranted,
    /// A gated call was refused — by a human, the denial ledger, or the
    /// unattended fail-closed rule.
    ApprovalDenied,
    /// An agent's signing key was minted.
    IdentityCreated,
    /// An agent's signing key was replaced; the old key is retired, not deleted.
    IdentityRotated,
    /// An agent's identity was revoked.
    IdentityRevoked,
}

impl LedgerAction {
    /// Stable wire/DB/preimage string. **Changing one invalidates every
    /// existing chain** — these strings are inside the hash.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ToolCall => "tool_call",
            Self::ToolDenied => "tool_denied",
            Self::ApprovalGranted => "approval_granted",
            Self::ApprovalDenied => "approval_denied",
            Self::IdentityCreated => "identity_created",
            Self::IdentityRotated => "identity_rotated",
            Self::IdentityRevoked => "identity_revoked",
        }
    }

    const ALL: &'static [Self] = &[
        Self::ToolCall,
        Self::ToolDenied,
        Self::ApprovalGranted,
        Self::ApprovalDenied,
        Self::IdentityCreated,
        Self::IdentityRotated,
        Self::IdentityRevoked,
    ];
}

impl fmt::Display for LedgerAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LedgerAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|a| a.as_str() == s)
            .ok_or_else(|| format!("unknown ledger action: {s:?}"))
    }
}

/// How the recorded action ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerOutcome {
    Ok,
    Error,
    Denied,
}

impl LedgerOutcome {
    /// Stable preimage string — see [`LedgerAction::as_str`].
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Denied => "denied",
        }
    }

    const ALL: &'static [Self] = &[Self::Ok, Self::Error, Self::Denied];
}

impl fmt::Display for LedgerOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LedgerOutcome {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|o| o.as_str() == s)
            .ok_or_else(|| format!("unknown ledger outcome: {s:?}"))
    }
}

/// Input to the ledger writer. `seq`, `prev_hash`, `hash`, `signature`,
/// `signer_fp` and `at_ms` are assigned by the writer, never by the caller.
///
/// Not `Deserialize` — see the module doc.
#[derive(Debug, Clone)]
pub struct NewRecord {
    /// The acting agent. Resolved at the chokepoint from `TURN_CONTEXT`, never
    /// taken from tool arguments.
    pub agent_id: String,
    pub action: LedgerAction,
    /// Tool name, or the subject of an identity event.
    pub target: String,
    pub outcome: LedgerOutcome,
    /// `grant_fingerprint(tool, canonical args)` — proves *which* call happened
    /// without storing the arguments themselves.
    pub args_fp: Option<String>,
    /// Human-readable context. **Must already be redacted** — the ledger is an
    /// operator-visible surface and an argument can carry a credential. Tool
    /// records use [`ApprovalAction::summary`](crate::sandbox::exec_approval::ApprovalAction),
    /// which is masked and capped at the single source.
    pub detail: String,
}

/// A materialized ledger row.
#[derive(Debug, Clone, Serialize)]
pub struct LedgerRecord {
    pub agent_id: String,
    pub seq: i64,
    #[serde(serialize_with = "ser_opt_hex")]
    pub prev_hash: Option<Vec<u8>>,
    #[serde(serialize_with = "ser_hex")]
    pub hash: Vec<u8>,
    #[serde(serialize_with = "ser_hex")]
    pub signature: Vec<u8>,
    /// Fingerprint of the key that signed this row. Rotation means a chain can
    /// legitimately contain more than one signer.
    pub signer_fp: String,
    pub action: LedgerAction,
    pub target: String,
    pub outcome: LedgerOutcome,
    pub args_fp: Option<String>,
    pub detail: String,
    pub at_ms: i64,
}

fn ser_hex<S: serde::Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&hex::encode(bytes))
}

fn ser_opt_hex<S: serde::Serializer>(bytes: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
    match bytes {
        Some(b) => s.serialize_str(&hex::encode(b)),
        None => s.serialize_none(),
    }
}

/// Decode a ledger row selected with [`LEDGER_COLUMNS`](super::schema::LEDGER_COLUMNS).
///
/// An unparseable `action`/`outcome` is a hard error rather than a lenient
/// fallback: a row the verifier cannot reconstruct byte-for-byte must not be
/// silently reconstructed as something else — that would turn a corrupted row
/// into a passing one.
pub fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<LedgerRecord> {
    let action_str: String = row.get(6)?;
    let outcome_str: String = row.get(8)?;
    let parse_err = |i: usize, e: String| {
        rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Text, e.into())
    };
    Ok(LedgerRecord {
        agent_id: row.get(0)?,
        seq: row.get(1)?,
        prev_hash: row.get(2)?,
        hash: row.get(3)?,
        signature: row.get(4)?,
        signer_fp: row.get(5)?,
        action: action_str.parse().map_err(|e| parse_err(6, e))?,
        target: row.get(7)?,
        outcome: outcome_str.parse().map_err(|e| parse_err(8, e))?,
        args_fp: row.get(9)?,
        detail: row.get(10)?,
        at_ms: row.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_strings_round_trip() {
        for a in LedgerAction::ALL {
            assert_eq!(&a.to_string().parse::<LedgerAction>().unwrap(), a);
        }
    }

    #[test]
    fn outcome_strings_round_trip() {
        for o in LedgerOutcome::ALL {
            assert_eq!(&o.to_string().parse::<LedgerOutcome>().unwrap(), o);
        }
    }

    #[test]
    fn unknown_strings_are_errors_not_defaults() {
        assert!("nope".parse::<LedgerAction>().is_err());
        assert!("nope".parse::<LedgerOutcome>().is_err());
    }
}
