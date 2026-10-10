//! Server-authored, versioned durable marker for [`TelecomPublicContext`].
//!
//! `TelecomPublicContext` deliberately has no serde surface. The busy-queue
//! journal nevertheless has to carry "this queued run came in on a public
//! telecom line" across a restart, or a reinjected run silently folds `Some`
//! into `None` (= legacy / "says nothing"). This DTO is that carrier.
//!
//! # Rules
//!
//! * The only writers are the durable queue payload, goal persistence, and
//!   the harness runner's resume snapshot writer (census test below).
//!   Nothing builds a marker from a metadata map, a string key, or
//!   `source_channel`.
//! * Reading never fails and never guesses: any JSON shape is classified into
//!   a state. Only an exact, known v1 shape restores a context; everything else
//!   is `Legacy` (field absent), `Unknown` (well-formed but unrecognised
//!   version/state/channel) or `Corrupt` (unparsable). `Unknown` / `Corrupt`
//!   keep the raw JSON so a tombstone rewrite does not erase the evidence.
//! * There is no integrity tag. The journal is in the same trust domain as the
//!   session transcript store; a local attacker who can edit it can already
//!   edit the queued input. This is not an unforgeable marker.
//!
//! # Known limit (blocks any consumer)
//!
//! A record journaled before this field existed reads back as
//! [`PersistedTelecomMarker::Legacy`]: it cannot prove it was *not* telecom
//! originated. Restoring it as `None` is the pre-existing behaviour, reported
//! via [`PersistedTelecomMarker::limitation`], but no consumer of
//! `telecom_public` may treat reinjected `None` as "not public" until legacy
//! records are drained or a safe refusal exists.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value};

use super::public_context::{TelecomChannel, TelecomPublicContext};

/// Current wire version of the marker.
pub const MARKER_VERSION: u64 = 1;

/// Fields of a recognised v1 public marker. Private: only the parser builds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedPublicV1 {
    channel: TelecomChannel,
    region: String,
    binding_ref: String,
    connection_generation: u64,
}

impl PersistedPublicV1 {
    pub(super) fn channel(&self) -> TelecomChannel {
        self.channel
    }
    pub(super) fn region(&self) -> &str {
        &self.region
    }
    pub(super) fn binding_ref(&self) -> &str {
        &self.binding_ref
    }
    pub(super) fn connection_generation(&self) -> u64 {
        self.connection_generation
    }
}

/// Durable classification of a run's telecom provenance.
#[derive(Clone, PartialEq, Eq)]
pub enum PersistedTelecomMarker {
    /// The record has no marker field (written before this field existed).
    /// Never serialised, so a tombstone rewrite keeps it legacy.
    Legacy,
    /// Written by this version for a run with `telecom_public == None`.
    Ordinary,
    /// Written by this version for a run with `telecom_public == Some`.
    Public(PersistedPublicV1),
    /// Well-formed JSON object this build does not recognise (newer version,
    /// unknown state or channel). Raw value kept verbatim.
    Unknown(Value),
    /// Not a parsable marker (wrong shape / missing or mistyped fields).
    Corrupt(Value),
}

impl Default for PersistedTelecomMarker {
    /// What a missing field deserialises to (`#[serde(default)]`).
    fn default() -> Self {
        Self::Legacy
    }
}

impl std::fmt::Debug for PersistedTelecomMarker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print region / binding_ref / raw payloads.
        let tag = match self {
            Self::Legacy => "Legacy",
            Self::Ordinary => "Ordinary",
            Self::Public(_) => "Public(<redacted>)",
            Self::Unknown(_) => "Unknown(<raw>)",
            Self::Corrupt(_) => "Corrupt(<raw>)",
        };
        f.write_str(tag)
    }
}

/// What reinjection may do with a journaled marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkerRestore {
    /// Run as ordinary (`telecom_public = None`). Marker was written `Ordinary`.
    Ordinary,
    /// Pre-marker record: restored as `None`, but see [`PersistedTelecomMarker::limitation`].
    LegacyUnproven,
    /// Restore this public context.
    Public(TelecomPublicContext),
    /// Unrecognised / corrupt: the run must NOT execute (reason attached).
    Refuse(String),
}

impl PersistedTelecomMarker {
    /// The single write entry: capture a live request's provenance.
    pub(crate) fn capture(ctx: Option<&TelecomPublicContext>) -> Self {
        match ctx {
            None => Self::Ordinary,
            Some(c) => {
                let p = c.provenance();
                Self::Public(PersistedPublicV1 {
                    channel: p.channel(),
                    region: p.region().to_string(),
                    binding_ref: p.binding_ref().to_string(),
                    connection_generation: p.connection_generation(),
                })
            }
        }
    }

    /// Decide what a reinjection does with this marker. Pure.
    pub(crate) fn restore(&self) -> MarkerRestore {
        match self {
            Self::Ordinary => MarkerRestore::Ordinary,
            Self::Legacy => MarkerRestore::LegacyUnproven,
            Self::Public(p) => MarkerRestore::Public(TelecomPublicContext::restore_v1(p)),
            Self::Unknown(_) => MarkerRestore::Refuse(
                "telecom provenance marker has an unrecognised version/state; refusing to run it as ordinary"
                    .into(),
            ),
            Self::Corrupt(_) => MarkerRestore::Refuse(
                "telecom provenance marker is unreadable; refusing to run it as ordinary".into(),
            ),
        }
    }

    /// Stated limitation of this marker's `restore`, if any.
    pub fn limitation(&self) -> Option<&'static str> {
        match self {
            Self::Legacy => Some(
                "journaled before telecom provenance was recorded: cannot prove the run was not telecom-originated; restored as None",
            ),
            _ => None,
        }
    }

    fn to_value(&self) -> Option<Value> {
        match self {
            Self::Legacy => None,
            Self::Ordinary => Some(json!({"version": MARKER_VERSION, "state": "ordinary"})),
            Self::Public(p) => Some(json!({
                "version": MARKER_VERSION,
                "state": "public",
                "channel": match p.channel {
                    TelecomChannel::Sms => "sms",
                    TelecomChannel::Voice => "voice",
                },
                "region": p.region,
                "binding_ref": p.binding_ref,
                "connection_generation": p.connection_generation,
            })),
            Self::Unknown(v) | Self::Corrupt(v) => Some(v.clone()),
        }
    }

    fn parse(v: Value) -> Self {
        let Some(obj) = v.as_object() else {
            return Self::Corrupt(v);
        };
        let Some(version) = obj.get("version").and_then(Value::as_u64) else {
            return Self::Corrupt(v);
        };
        if version != MARKER_VERSION {
            return Self::Unknown(v);
        }
        match obj.get("state").and_then(Value::as_str) {
            Some("ordinary") => Self::Ordinary,
            Some("public") => {
                let channel = match obj.get("channel").and_then(Value::as_str) {
                    Some("sms") => TelecomChannel::Sms,
                    Some("voice") => TelecomChannel::Voice,
                    Some(_) => return Self::Unknown(v),
                    None => return Self::Corrupt(v),
                };
                let region = obj.get("region").and_then(Value::as_str);
                let binding = obj.get("binding_ref").and_then(Value::as_str);
                let generation = obj.get("connection_generation").and_then(Value::as_u64);
                match (region, binding, generation) {
                    (Some(r), Some(b), Some(g)) => Self::Public(PersistedPublicV1 {
                        channel,
                        region: r.to_string(),
                        binding_ref: b.to_string(),
                        connection_generation: g,
                    }),
                    _ => Self::Corrupt(v),
                }
            }
            Some(_) => Self::Unknown(v),
            None => Self::Corrupt(v),
        }
    }
}

impl Serialize for PersistedTelecomMarker {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // `Legacy` is skipped by the container (`skip_serializing_if`); if it is
        // ever serialised standalone it is an explicit null, which re-parses as
        // `Corrupt` rather than silently becoming `Ordinary`.
        self.to_value().unwrap_or(Value::Null).serialize(s)
    }
}

impl<'de> Deserialize<'de> for PersistedTelecomMarker {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Any JSON classifies; this never errors on shape.
        Ok(Self::parse(Value::deserialize(d)?))
    }
}

impl PersistedTelecomMarker {
    /// `skip_serializing_if` helper for the journal payload.
    pub fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::telecom::SmsInboundMeta;
    use aleph_protocol::telecom_relay::SmsEncoding;
    use chrono::Utc;

    fn ctx() -> TelecomPublicContext {
        TelecomPublicContext::from_admitted_sms(&SmsInboundMeta {
            region: "r-secret".into(),
            binding_ref: "b-secret".into(),
            provider_account_id: "acct".into(),
            provider_message_id: "pm".into(),
            envelope_event_id: "ev".into(),
            connection_generation: 9,
            service_address: "+15550001111".into(),
            peer_address: "+15552223333".into(),
            encoding: SmsEncoding::Gsm7,
            segment_count: 1,
            created_at: Utc::now(),
            expires_at: Utc::now(),
        })
    }

    fn roundtrip(m: &PersistedTelecomMarker) -> PersistedTelecomMarker {
        serde_json::from_slice(&serde_json::to_vec(m).unwrap()).unwrap()
    }

    #[test]
    fn some_survives_capture_serde_and_restore() {
        let c = ctx();
        let m = roundtrip(&PersistedTelecomMarker::capture(Some(&c)));
        match m.restore() {
            MarkerRestore::Public(back) => assert_eq!(back, c),
            other => panic!("expected Public, got {other:?}"),
        }
    }

    #[test]
    fn none_is_written_explicitly_as_ordinary_not_as_absence() {
        let m = roundtrip(&PersistedTelecomMarker::capture(None));
        assert_eq!(m, PersistedTelecomMarker::Ordinary);
        assert_eq!(m.restore(), MarkerRestore::Ordinary);
        assert!(m.limitation().is_none());
    }

    #[test]
    fn legacy_is_distinct_reports_a_limitation_and_is_never_serialised_as_ordinary() {
        let m = PersistedTelecomMarker::default();
        assert_eq!(m, PersistedTelecomMarker::Legacy);
        assert!(m.limitation().is_some());
        assert_eq!(m.restore(), MarkerRestore::LegacyUnproven);
        assert!(m.is_legacy());
        // Standalone serialisation must not launder legacy into ordinary.
        let back: PersistedTelecomMarker =
            serde_json::from_slice(&serde_json::to_vec(&m).unwrap()).unwrap();
        assert!(matches!(back, PersistedTelecomMarker::Corrupt(_)));
    }

    #[test]
    fn unknown_version_state_and_channel_are_refused_and_keep_raw_json() {
        for raw in [
            json!({"version": 2, "state": "public"}),
            json!({"version": 1, "state": "quarantine"}),
            json!({"version": 1, "state": "public", "channel": "fax",
                   "region": "r", "binding_ref": "b", "connection_generation": 1}),
        ] {
            let m: PersistedTelecomMarker = serde_json::from_value(raw.clone()).unwrap();
            assert!(matches!(m, PersistedTelecomMarker::Unknown(_)), "{raw}");
            assert!(matches!(m.restore(), MarkerRestore::Refuse(_)));
            assert_eq!(m.to_value(), Some(raw), "raw evidence must round-trip");
        }
    }

    #[test]
    fn corrupt_shapes_are_refused_never_defaulted_to_ordinary() {
        for raw in [
            json!(null),
            json!("public"),
            json!(true),
            json!([]),
            json!({}),
            json!({"version": "1", "state": "ordinary"}),
            json!({"version": 1}),
            json!({"version": 1, "state": "public"}),
            json!({"version": 1, "state": "public", "channel": "sms", "region": "r"}),
            json!({"version": 1, "state": "public", "channel": "sms", "region": 1,
                   "binding_ref": "b", "connection_generation": 1}),
        ] {
            let m: PersistedTelecomMarker = serde_json::from_value(raw.clone()).unwrap();
            assert!(matches!(m, PersistedTelecomMarker::Corrupt(_)), "{raw}");
            assert!(matches!(m.restore(), MarkerRestore::Refuse(_)), "{raw}");
        }
    }

    #[test]
    fn debug_never_prints_line_binding() {
        let m = PersistedTelecomMarker::capture(Some(&ctx()));
        let dbg = format!("{m:?}");
        assert!(
            !dbg.contains("r-secret") && !dbg.contains("b-secret"),
            "{dbg}"
        );
    }

    /// `capture` is restricted to the three server-authored persistence paths:
    /// durable queue payloads, goal storage, and the harness runner's resume
    /// snapshot writer. The latter must preserve telecom provenance when a
    /// resumed run is re-journaled.
    #[test]
    fn capture_has_exactly_three_production_callers() {
        fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, hits);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let s = std::fs::read_to_string(&p).unwrap();
                    // Only the part before `#[cfg(test)] mod tests`.
                    let prod = s.split("#[cfg(test)]").next().unwrap_or("");
                    if prod.contains(concat!("PersistedTelecomMarker", "::capture(")) {
                        hits.push(p.display().to_string());
                    }
                }
            }
        }
        let mut hits = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut hits,
        );
        hits.sort();
        assert_eq!(hits.len(), 3, "{hits:?}");
        assert!(hits[0].ends_with("busy_queue/durable.rs"), "{hits:?}");
        assert!(hits[1].ends_with("goal/store.rs"), "{hits:?}");
        assert!(
            hits[2].ends_with("orchestrator/harness_bridge/runner_impl.rs"),
            "{hits:?}"
        );
    }
}
