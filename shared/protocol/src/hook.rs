//! Strict Hook@1 wire and pure fact validation. IDs correlate facts; they grant no authority.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fmt;

pub const HOOK_CONTRACT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RequestWire")]
pub struct HookEnvelope {
    pub version: u32,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub slot: String,
    pub event: HookInput,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestWire {
    version: u32,
    session_id: String,
    turn_id: Option<String>,
    slot: String,
    event: HookInput,
}

impl TryFrom<RequestWire> for HookEnvelope {
    type Error = HookWireError;

    fn try_from(wire: RequestWire) -> Result<Self, Self::Error> {
        let envelope = Self {
            version: wire.version,
            session_id: wire.session_id,
            turn_id: wire.turn_id,
            slot: wire.slot,
            event: wire.event,
        };
        validate_envelope(&envelope)?;
        Ok(envelope)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ResponseWire")]
pub struct HookResponseEnvelope {
    pub version: u32,
    pub response: HookResponse,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponseWire {
    version: u32,
    response: HookResponse,
}

impl TryFrom<ResponseWire> for HookResponseEnvelope {
    type Error = HookWireError;

    fn try_from(wire: ResponseWire) -> Result<Self, Self::Error> {
        validate_version(wire.version)?;
        Ok(Self {
            version: wire.version,
            response: wire.response,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEventTag {
    #[serde(rename = "tool.pre")]
    ToolPre,
    #[serde(rename = "tool.post")]
    ToolPost,
    #[serde(rename = "prompt")]
    Prompt,
    #[serde(rename = "compaction")]
    Compaction,
    #[serde(rename = "continue")]
    Continue,
    #[serde(rename = "ingest_filter")]
    IngestFilter,
    #[serde(rename = "observer")]
    Observer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", deny_unknown_fields)]
pub enum HookInput {
    #[serde(rename = "tool.pre")]
    ToolPre {
        call_id: String,
        tool: String,
        input: Value,
        original_input: Value,
    },
    #[serde(rename = "tool.post")]
    ToolPost {
        call_id: String,
        tool: String,
        result: Value,
    },
    #[serde(rename = "prompt")]
    Prompt { layers: Vec<PromptLayerView> },
    #[serde(rename = "compaction")]
    Compaction {
        messages: Vec<CompactionMessage>,
        required_pairs: Vec<ToolPair>,
        required_session_markers: Vec<String>,
    },
    #[serde(rename = "continue")]
    Continue { iteration: u32, limit: u32 },
    #[serde(rename = "ingest_filter")]
    IngestFilter { records: Vec<IngestRecord> },
    #[serde(rename = "observer")]
    Observer { lifecycle: ObserverLifecycle },
}

impl HookInput {
    pub const fn event_tag(&self) -> HookEventTag {
        match self {
            Self::ToolPre { .. } => HookEventTag::ToolPre,
            Self::ToolPost { .. } => HookEventTag::ToolPost,
            Self::Prompt { .. } => HookEventTag::Prompt,
            Self::Compaction { .. } => HookEventTag::Compaction,
            Self::Continue { .. } => HookEventTag::Continue,
            Self::IngestFilter { .. } => HookEventTag::IngestFilter,
            Self::Observer { .. } => HookEventTag::Observer,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", deny_unknown_fields)]
pub enum HookResponse {
    #[serde(rename = "tool.pre")]
    ToolPre { action: ToolPreAction },
    #[serde(rename = "tool.post")]
    ToolPost { replacement: Option<Value> },
    #[serde(rename = "prompt")]
    Prompt {
        append: Vec<String>,
        replace: Vec<ReplaceLayerRequest>,
    },
    #[serde(rename = "compaction")]
    Compaction { summary: CompactionSummary },
    #[serde(rename = "continue")]
    Continue {
        request: bool,
        reason: Option<String>,
    },
    #[serde(rename = "ingest_filter")]
    IngestFilter { keep_ids: Vec<String> },
    #[serde(rename = "observer")]
    Observer { ack: ObserverAck },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolPreAction {
    Continue { updated_input: Option<Value> },
    Deny { reason: String },
    Ask { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptLayerView {
    pub id: String,
    pub text: String,
    pub replaceability: Replaceability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Replaceability {
    Pinned,
    Replaceable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceLayerRequest {
    pub replaceable_layer_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionMessage {
    pub id: String,
    pub role: String,
    pub text: String,
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPair {
    pub call_id: String,
    pub result_message_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionSummary {
    pub text: String,
    pub preserved_pairs: Vec<ToolPair>,
    pub session_markers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestRecord {
    pub id: String,
    pub content: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLifecycleContext {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnLifecycleContext {
    pub session_id: String,
    pub turn_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionEndContext {
    pub session_id: String,
    pub reason: String,
    pub outcome: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnEndContext {
    pub session_id: String,
    pub turn_id: String,
    pub reason: String,
    pub outcome: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ObserverLifecycle {
    SessionStart { context: SessionLifecycleContext },
    SessionEnd { context: SessionEndContext },
    TurnStart { context: TurnLifecycleContext },
    TurnEnd { context: TurnEndContext },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObserverAck {
    pub records: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookWireError {
    UnsupportedVersion(u32),
    Malformed(String),
    EventMismatch,
    InvalidResponse(String),
}

impl fmt::Display for HookWireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(v) => write!(f, "unsupported hook contract version: {v}"),
            Self::Malformed(message) => write!(f, "malformed hook request: {message}"),
            Self::EventMismatch => f.write_str("hook response event mismatch"),
            Self::InvalidResponse(message) => write!(f, "invalid hook response: {message}"),
        }
    }
}
impl std::error::Error for HookWireError {}

pub fn decode_hook_v1(json: &str) -> Result<HookEnvelope, HookWireError> {
    serde_json::from_str::<RequestWire>(json)
        .map_err(|e| HookWireError::Malformed(e.to_string()))?
        .try_into()
}

pub fn decode_hook_response_v1(json: &str) -> Result<HookResponseEnvelope, HookWireError> {
    serde_json::from_str::<ResponseWire>(json)
        .map_err(|e| HookWireError::Malformed(e.to_string()))?
        .try_into()
}

fn validate_version(version: u32) -> Result<(), HookWireError> {
    if version == HOOK_CONTRACT_VERSION {
        Ok(())
    } else {
        Err(HookWireError::UnsupportedVersion(version))
    }
}

fn nonempty(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty() {
        Err(format!("{field} must be nonempty"))
    } else {
        Ok(())
    }
}

/// Validate a directly constructed Rust envelope without serialization.
/// This pure compatibility entry point is shared with the future typed request guard.
#[doc(hidden)]
pub fn validate_envelope(envelope: &HookEnvelope) -> Result<(), HookWireError> {
    validate_version(envelope.version)?;
    validate_envelope_facts(envelope).map_err(HookWireError::Malformed)
}

fn validate_envelope_facts(e: &HookEnvelope) -> Result<(), String> {
    nonempty(&e.session_id, "session_id")?;
    nonempty(&e.slot, "slot")?;
    let session_lifecycle = matches!(
        &e.event,
        HookInput::Observer {
            lifecycle: ObserverLifecycle::SessionStart { .. }
                | ObserverLifecycle::SessionEnd { .. }
        }
    );
    if session_lifecycle {
        if e.turn_id.is_some() {
            return Err("session lifecycle must have no turn_id".into());
        }
    } else {
        nonempty(
            e.turn_id.as_deref().ok_or("turn_id is required")?,
            "turn_id",
        )?;
    }
    if let HookInput::Observer { lifecycle } = &e.event {
        let (session_id, turn_id) = match lifecycle {
            ObserverLifecycle::SessionStart { context } => (&context.session_id, None),
            ObserverLifecycle::SessionEnd { context } => (&context.session_id, None),
            ObserverLifecycle::TurnStart { context } => {
                (&context.session_id, Some(&context.turn_id))
            }
            ObserverLifecycle::TurnEnd { context } => (&context.session_id, Some(&context.turn_id)),
        };
        nonempty(session_id, "context.session_id")?;
        if session_id != &e.session_id {
            return Err("lifecycle session_id mismatch".into());
        }
        if let Some(turn_id) = turn_id {
            nonempty(turn_id, "context.turn_id")?;
            if Some(turn_id.as_str()) != e.turn_id.as_deref() {
                return Err("lifecycle turn_id mismatch".into());
            }
        }
    }
    validate_input(&e.event)
}

fn unique_strings<'a>(
    values: impl IntoIterator<Item = &'a str>,
    field: &str,
) -> Result<HashSet<&'a str>, String> {
    let mut set = HashSet::new();
    for value in values {
        if !set.insert(value) {
            return Err(format!("duplicate {field}"));
        }
    }
    Ok(set)
}

fn pair_set(pairs: &[ToolPair]) -> Result<HashSet<ToolPair>, String> {
    let mut calls = HashSet::new();
    let mut results = HashSet::new();
    let mut set = HashSet::new();
    for pair in pairs {
        nonempty(&pair.call_id, "pair.call_id")?;
        nonempty(&pair.result_message_id, "pair.result_message_id")?;
        if !calls.insert(&pair.call_id)
            || !results.insert(&pair.result_message_id)
            || !set.insert(pair.clone())
        {
            return Err("duplicate pair call_id or result_message_id".into());
        }
    }
    Ok(set)
}

fn compaction_pairs(messages: &[CompactionMessage]) -> Result<HashSet<ToolPair>, String> {
    unique_strings(messages.iter().map(|m| m.id.as_str()), "message id")?;
    let mut calls = HashSet::new();
    let mut results = HashMap::new();
    for message in messages {
        nonempty(&message.id, "message.id")?;
        match message.role.as_str() {
            "user" | "assistant" | "system" => {}
            "tool_call" | "tool_result" | "tool_error" => {
                let call_id = message
                    .tool_call_id
                    .as_deref()
                    .ok_or("tool row requires tool_call_id")?;
                nonempty(call_id, "tool_call_id")?;
                if message.role == "tool_call" {
                    if !calls.insert(call_id) {
                        return Err("ambiguous repeated tool call".into());
                    }
                } else if results.insert(call_id, message.id.as_str()).is_some() {
                    return Err("ambiguous repeated tool result".into());
                }
            }
            _ => return Err("unknown compaction message role".into()),
        }
    }
    Ok(calls
        .into_iter()
        .filter_map(|call_id| {
            results.get(call_id).map(|result| ToolPair {
                call_id: call_id.into(),
                result_message_id: (*result).into(),
            })
        })
        .collect())
}

fn validate_input(input: &HookInput) -> Result<(), String> {
    match input {
        HookInput::ToolPre { call_id, tool, .. } | HookInput::ToolPost { call_id, tool, .. } => {
            nonempty(call_id, "call_id")?;
            nonempty(tool, "tool")?;
        }
        HookInput::Prompt { layers } => {
            unique_strings(layers.iter().map(|l| l.id.as_str()), "layer id")?;
            for layer in layers {
                nonempty(&layer.id, "layer.id")?;
            }
        }
        HookInput::Compaction {
            messages,
            required_pairs,
            required_session_markers,
        } => {
            let real = compaction_pairs(messages)?;
            if pair_set(required_pairs)? != real {
                return Err("required_pairs must equal all real pairs".into());
            }
            unique_strings(
                required_session_markers.iter().map(String::as_str),
                "required session marker",
            )?;
        }
        HookInput::IngestFilter { records } => {
            unique_strings(records.iter().map(|r| r.id.as_str()), "record id")?;
            for record in records {
                nonempty(&record.id, "record.id")?;
            }
        }
        HookInput::Observer { lifecycle } => match lifecycle {
            ObserverLifecycle::SessionEnd { context } => {
                nonempty(&context.reason, "reason")?;
                nonempty(&context.outcome, "outcome")?;
            }
            ObserverLifecycle::TurnEnd { context } => {
                nonempty(&context.reason, "reason")?;
                nonempty(&context.outcome, "outcome")?;
            }
            _ => {}
        },
        HookInput::Continue { .. } => {}
    }
    Ok(())
}

pub fn validate_response(input: &HookInput, response: &HookResponse) -> Result<(), HookWireError> {
    validate_input(input).map_err(HookWireError::Malformed)?;
    let invalid = |s: String| HookWireError::InvalidResponse(s);
    match (input, response) {
        (HookInput::ToolPre { .. }, HookResponse::ToolPre { .. })
        | (HookInput::ToolPost { .. }, HookResponse::ToolPost { .. })
        | (HookInput::Observer { .. }, HookResponse::Observer { .. }) => Ok(()),
        (HookInput::Prompt { layers }, HookResponse::Prompt { replace, .. }) => {
            unique_strings(
                replace.iter().map(|r| r.replaceable_layer_id.as_str()),
                "replace layer id",
            )
            .map_err(invalid)?;
            for request in replace {
                if !layers.iter().any(|l| {
                    l.id == request.replaceable_layer_id
                        && l.replaceability == Replaceability::Replaceable
                }) {
                    return Err(invalid(
                        "replacement requires a declared Replaceable layer".into(),
                    ));
                }
            }
            Ok(())
        }
        (
            HookInput::Compaction {
                required_pairs,
                required_session_markers,
                ..
            },
            HookResponse::Compaction { summary },
        ) => {
            if pair_set(&summary.preserved_pairs).map_err(invalid)?
                != pair_set(required_pairs).map_err(invalid)?
            {
                return Err(invalid(
                    "preserved_pairs must equal all required real pairs".into(),
                ));
            }
            let expected = unique_strings(
                required_session_markers.iter().map(String::as_str),
                "required session marker",
            )
            .map_err(invalid)?;
            if unique_strings(
                summary.session_markers.iter().map(String::as_str),
                "session marker",
            )
            .map_err(invalid)?
                != expected
            {
                return Err(invalid(
                    "session_markers must preserve the exact opaque marker set".into(),
                ));
            }
            Ok(())
        }
        (HookInput::Continue { iteration, limit }, HookResponse::Continue { request, .. }) => {
            if *request && iteration >= limit {
                Err(invalid("continue request reached limit".into()))
            } else {
                Ok(())
            }
        }
        (HookInput::IngestFilter { records }, HookResponse::IngestFilter { keep_ids }) => {
            let kept =
                unique_strings(keep_ids.iter().map(String::as_str), "keep id").map_err(invalid)?;
            let available: HashSet<&str> = records.iter().map(|r| r.id.as_str()).collect();
            if kept.is_subset(&available) {
                Ok(())
            } else {
                Err(invalid("keep_ids must be an input ID subset".into()))
            }
        }
        _ => Err(HookWireError::EventMismatch),
    }
}
