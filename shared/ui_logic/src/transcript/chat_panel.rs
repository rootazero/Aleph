//! NSH Chat's transport-neutral state and render patches.

use std::collections::{HashMap, HashSet};

use aleph_protocol::chat_transcript::{ChatToolChange, ChatToolDelta, ChatTranscriptDetails};
use aleph_protocol::StreamEvent;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Change, RowBody, RowStatus, StepStatus, Transcript, TranscriptEntry};

const EARLY_FRAMES: usize = 64;
const EARLY_BYTES: usize = 256 * 1024;
const EARLY_AGE_MS: u64 = 10_000;

#[derive(Default)]
pub struct ChatState {
    session: String,
    live: Option<LiveRun>,
    sealed: Option<LiveRun>,
    pending: Option<PendingSend>,
    last_ended: Option<String>,
    committed_diffs: HashMap<String, RowBody>,
    generation: u64,
}

struct LiveRun {
    id: String,
    fold: Transcript,
    text_keys: HashMap<String, String>,
    omitted: HashSet<String>,
    last_seq: Option<u64>,
    missing_prefix: bool,
}

struct PendingSend {
    key: String,
    text: String,
    run: Option<String>,
    started_ms: u64,
    held: Vec<ChatFrame>,
    bytes: usize,
    incomplete: bool,
}

#[derive(Deserialize)]
struct History {
    session_key: String,
    #[serde(default)]
    messages: Vec<Message>,
    active_run: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    role: String,
    content: String,
    timestamp: Option<String>,
    run_id: Option<String>,
}

#[derive(Deserialize)]
struct ChatFrame {
    topic: String,
    data: FrameData,
}

#[derive(Deserialize)]
struct FrameData {
    run_id: String,
    seq: Option<u64>,
    delta: Option<Value>,
    content: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Row {
    Text {
        key: String,
        role: String,
        text: String,
        streaming: bool,
    },
    Tool {
        key: String,
        run_id: String,
        tool_call_id: String,
        display_name: String,
        args_text: String,
        status: &'static str,
        duration_ms: Option<u64>,
        has_diff: bool,
        presentation_omitted: bool,
    },
}

impl Row {
    fn key(&self) -> &str {
        match self {
            Self::Text { key, .. } | Self::Tool { key, .. } => key,
        }
    }
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Patch {
    upsert: Vec<Row>,
    remove: Vec<String>,
    append_order: Vec<String>,
    reconcile: bool,
    active_run: Option<String>,
    missing_prefix: bool,
    error: Option<String>,
}

impl Patch {
    fn insert(&mut self, row: Row) {
        self.append_order.push(row.key().to_owned());
        self.upsert.push(row);
    }

    fn update(&mut self, row: Row) {
        if let Some(previous) = self
            .upsert
            .iter_mut()
            .find(|prior| prior.key() == row.key())
        {
            *previous = row;
        } else {
            self.upsert.push(row);
        }
    }
}

impl ChatState {
    pub fn reset(&mut self, session: String) {
        *self = Self {
            session,
            ..Self::default()
        };
    }

    pub fn begin_send(&mut self, text: String, now_ms: u64) -> Value {
        let mut patch = self.patch();
        if self.live.is_some()
            || self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.run.is_none())
        {
            return encode(patch);
        }
        if let Some(previous) = self.pending.take() {
            patch.remove.push(previous.key);
        }
        self.generation += 1;
        let pending = PendingSend {
            key: format!("local/{}", self.generation),
            text,
            run: None,
            started_ms: now_ms,
            held: Vec::new(),
            bytes: 0,
            incomplete: false,
        };
        patch.insert(pending.row());
        self.pending = Some(pending);
        encode(patch)
    }

    pub fn resolve_send(&mut self, session: String, run: Option<String>, now_ms: u64) -> Value {
        let mut patch = self.patch();
        let Some(mut pending) = self.pending.take() else {
            return encode(patch);
        };
        let Some(run_id) = run else {
            patch.remove.push(pending.key);
            return encode(patch);
        };
        self.session = session;
        let held = std::mem::take(&mut pending.held);
        let expired = now_ms.saturating_sub(pending.started_ms) > EARLY_AGE_MS;
        let missing = pending.incomplete || expired;
        pending.run = Some(run_id.clone());
        self.pending = Some(pending);
        self.live = Some(LiveRun::new(run_id.clone(), now_ms, missing));
        if !expired {
            for frame in held.into_iter().filter(|frame| frame.data.run_id == run_id) {
                self.fold_frame(frame, now_ms, &mut patch);
            }
        }
        patch.reconcile |= missing;
        self.finish_patch(&mut patch);
        encode(patch)
    }

    pub fn accept(&mut self, event: Value) -> Value {
        let now_ms = event.get("nowMs").and_then(Value::as_u64).unwrap_or(0);
        let bytes = if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.run.is_none())
        {
            serde_json::to_vec(&event).map_or(usize::MAX, |value| value.len())
        } else {
            0
        };
        let mut patch = self.patch();
        let Ok(frame) = serde_json::from_value::<ChatFrame>(event) else {
            return encode(patch);
        };
        if !matches!(
            frame.topic.as_str(),
            "stream.response_chunk"
                | "stream.chat_tool"
                | "stream.run_complete"
                | "stream.run_error"
        ) {
            return encode(patch);
        }
        if self
            .live
            .as_ref()
            .is_some_and(|run| run.id == frame.data.run_id)
        {
            self.fold_frame(frame, now_ms, &mut patch);
        } else if let Some(pending) = self
            .pending
            .as_mut()
            .filter(|pending| pending.run.is_none())
        {
            // Keep complete frames only and never retain unrelated data after the send reply.
            if now_ms.saturating_sub(pending.started_ms) > EARLY_AGE_MS {
                pending.held.clear();
                pending.incomplete = true;
            } else if pending.held.len() < EARLY_FRAMES
                && pending.bytes.saturating_add(bytes) <= EARLY_BYTES
            {
                pending.bytes += bytes;
                pending.held.push(frame);
            } else {
                pending.incomplete = true;
            }
        }
        self.finish_patch(&mut patch);
        encode(patch)
    }

    pub fn interrupt(&mut self) -> Value {
        if let Some(run) = &mut self.live {
            run.missing_prefix = true;
        }
        if let Some(pending) = &mut self.pending {
            pending.incomplete = true;
        }
        encode(self.patch())
    }

    pub fn replace_history(&mut self, history: Value, details: Value) -> Value {
        let Ok(history) = serde_json::from_value::<History>(history) else {
            return Value::Null;
        };
        if self.session != history.session_key {
            return Value::Null;
        }
        let mut details = serde_json::from_value::<ChatTranscriptDetails>(details).unwrap_or(
            ChatTranscriptDetails {
                runs: Vec::new(),
                limited: false,
            },
        );
        let assistant_runs: HashSet<_> = history
            .messages
            .iter()
            .filter(|message| message.role == "assistant")
            .filter_map(|message| message.run_id.as_deref())
            .collect();
        if self
            .sealed
            .as_ref()
            .is_some_and(|run| assistant_runs.contains(run.id.as_str()))
        {
            self.sealed = None;
        }
        if self
            .live
            .as_ref()
            .is_some_and(|run| history.active_run.as_deref() != Some(&run.id))
        {
            if let Some(mut run) = self.live.take() {
                self.last_ended = Some(run.id.clone());
                if !assistant_runs.contains(run.id.as_str()) {
                    run.fold.seal_chat_run(&run.id, 0);
                    self.sealed = Some(run);
                }
            }
        }
        if self.live.is_none() {
            if let Some(run) = history
                .active_run
                .filter(|id| Some(id) != self.last_ended.as_ref())
            {
                self.live = Some(LiveRun::new(run, 0, true));
            }
        }
        if let Some(pending) = &self.pending {
            if history.messages.iter().any(|message| {
                message.role == "user" && pending.run.is_some() && message.run_id == pending.run
            }) || pending
                .run
                .as_ref()
                .is_some_and(|run| assistant_runs.contains(run.as_str()))
            {
                self.pending = None;
            }
        }

        let provisional: HashSet<_> = self
            .live
            .iter()
            .chain(self.sealed.iter())
            .map(|run| run.id.as_str())
            .collect();
        let mut replay = HashMap::new();
        for group in details
            .runs
            .drain(..)
            .filter(|group| !provisional.contains(group.run_id.as_str()))
        {
            let mut run = LiveRun::new(group.run_id, 0, false);
            for delta in group.tools {
                run.fold_tool(&delta, 0);
            }
            run.fold.seal_chat_run(&run.id, 0);
            replay.insert(run.id.clone(), run);
        }
        let mut last_stamped_message = HashMap::new();
        let mut first_assistant = HashMap::new();
        for (index, message) in history.messages.iter().enumerate() {
            if let Some(run) = message.run_id.as_deref() {
                last_stamped_message.insert(run, index);
                if message.role == "assistant" {
                    first_assistant.entry(run).or_insert(index);
                }
            }
        }
        self.committed_diffs.clear();
        let mut rows = Vec::new();
        let mut occurrences = HashMap::<String, usize>::new();
        for (index, message) in history.messages.iter().enumerate() {
            let run = message.run_id.as_deref().unwrap_or("");
            let group = first_assistant
                .get(run)
                .or_else(|| last_stamped_message.get(run))
                .copied()
                == Some(index);
            if group && message.role == "assistant" {
                append_replay(run, &mut replay, &mut rows, &mut self.committed_diffs);
            }
            let key = format!(
                "history/{run}/{}/{}",
                message.role,
                message.timestamp.as_deref().unwrap_or("")
            );
            let occurrence = occurrences.entry(key.clone()).or_default();
            rows.push(Row::Text {
                key: format!("{key}/{occurrence}"),
                role: message.role.clone(),
                text: message.content.clone(),
                streaming: false,
            });
            *occurrence += 1;
            if group && message.role != "assistant" {
                append_replay(run, &mut replay, &mut rows, &mut self.committed_diffs);
            }
        }
        if let Some(run) = &self.sealed {
            rows.extend(run.rows());
        }
        if let Some(pending) = &self.pending {
            rows.push(pending.row());
        }
        if let Some(run) = &self.live {
            rows.extend(run.rows());
        }
        json!({"rows": rows, "activeRun": self.live.as_ref().map(|run| &run.id),
            "missingPrefix": self.live.as_ref().is_some_and(|run| run.missing_prefix), "limited": details.limited})
    }

    pub fn diff_body(&self, key: &str) -> Option<&[aleph_protocol::FileChange]> {
        let body = self
            .live
            .iter()
            .chain(self.sealed.iter())
            .find_map(|run| {
                run.fold.iter().find_map(|entry| match entry {
                    TranscriptEntry::Tool(row) if row.id == key => Some(&row.body),
                    _ => None,
                })
            })
            .or_else(|| self.committed_diffs.get(key));
        match body {
            Some(RowBody::FileChanges(changes)) => Some(changes),
            _ => None,
        }
    }

    fn patch(&self) -> Patch {
        let mut patch = Patch::default();
        self.finish_patch(&mut patch);
        patch
    }

    fn finish_patch(&self, patch: &mut Patch) {
        patch.active_run = self.live.as_ref().map(|run| run.id.clone());
        patch.missing_prefix = self.live.as_ref().is_some_and(|run| run.missing_prefix);
    }

    fn fold_frame(&mut self, frame: ChatFrame, now_ms: u64, patch: &mut Patch) {
        let Some(run) = self.live.as_mut() else {
            return;
        };
        let data = frame.data;
        if data
            .seq
            .is_some_and(|seq| run.last_seq.is_some_and(|last| last >= seq))
        {
            return;
        }
        if data.seq.is_some() {
            run.last_seq = data.seq;
        }
        let changes = match frame.topic.as_str() {
            "stream.response_chunk" => {
                let text = data
                    .content
                    .or_else(|| {
                        data.delta
                            .and_then(|delta| delta.as_str().map(str::to_owned))
                    })
                    .unwrap_or_default();
                run.fold.apply_live(
                    &StreamEvent::ResponseChunk {
                        run_id: run.id.clone(),
                        seq: data.seq.unwrap_or(0),
                        content: text,
                        chunk_index: 0,
                        is_final: false,
                        is_intermediate: false,
                    },
                    now_ms,
                )
            }
            "stream.chat_tool" => data
                .delta
                .and_then(|delta| serde_json::from_value::<ChatToolDelta>(delta).ok())
                .map_or_else(Vec::new, |delta| run.fold_tool(&delta, now_ms)),
            _ => {
                patch.reconcile = true;
                if frame.topic == "stream.run_error" {
                    patch.error = Some(data.error.unwrap_or_else(|| "unknown error".into()));
                }
                run.fold.seal_chat_run(&run.id, now_ms)
            }
        };
        run.project_changes(changes, data.seq, patch);
        if patch.reconcile
            && matches!(
                frame.topic.as_str(),
                "stream.run_complete" | "stream.run_error"
            )
        {
            self.last_ended = Some(run.id.clone());
            if let Some(previous) = self.sealed.take() {
                patch
                    .remove
                    .extend(previous.rows().map(|row| row.key().to_owned()));
            }
            self.sealed = self.live.take();
        }
    }
}

impl PendingSend {
    fn row(&self) -> Row {
        Row::Text {
            key: self.key.clone(),
            role: "user".into(),
            text: self.text.clone(),
            streaming: false,
        }
    }
}

impl LiveRun {
    fn new(id: String, now_ms: u64, missing_prefix: bool) -> Self {
        let mut fold = Transcript::new();
        fold.adopt_live_run(&id, now_ms);
        Self {
            id,
            fold,
            text_keys: HashMap::new(),
            omitted: HashSet::new(),
            last_seq: None,
            missing_prefix,
        }
    }

    fn fold_tool(&mut self, delta: &ChatToolDelta, now_ms: u64) -> Vec<Change> {
        let changes = self.fold.apply_chat_tool(&self.id, delta, now_ms);
        if !changes.is_empty()
            && matches!(
                delta.change,
                ChatToolChange::Finished {
                    presentation_omitted: true,
                    ..
                }
            )
        {
            self.omitted.insert(delta.tool_call_id.clone());
        }
        changes
    }

    fn rows(&self) -> impl Iterator<Item = Row> + '_ {
        self.fold.iter().filter_map(|entry| self.row(entry))
    }

    fn row(&self, entry: &TranscriptEntry) -> Option<Row> {
        match entry {
            TranscriptEntry::Tool(row) => {
                let tool_call_id = row.id.splitn(3, '/').nth(2)?.to_owned();
                let (status, duration_ms) = match row.status {
                    RowStatus::Pending => ("pending", None),
                    RowStatus::Running { .. } => ("running", None),
                    RowStatus::Ok { duration_ms } => ("ok", Some(duration_ms)),
                    RowStatus::Err { duration_ms, .. } => ("error", Some(duration_ms)),
                };
                Some(Row::Tool {
                    key: row.id.clone(),
                    run_id: self.id.clone(),
                    display_name: row.summary.display_name.clone(),
                    args_text: row.summary.args_text.clone(),
                    status,
                    duration_ms,
                    has_diff: matches!(row.body, RowBody::FileChanges(_)),
                    presentation_omitted: self.omitted.contains(&tool_call_id),
                    tool_call_id,
                })
            }
            TranscriptEntry::Step(step) => Some(Row::Text {
                key: self.text_keys.get(&step.id)?.clone(),
                role: "assistant".into(),
                text: step.text.clone()?,
                streaming: step.status == StepStatus::Live,
            }),
            _ => None,
        }
    }

    fn project_changes(&mut self, changes: Vec<Change>, seq: Option<u64>, patch: &mut Patch) {
        for change in changes {
            let (id, inserted) = match change {
                Change::NeedsResync => {
                    patch.reconcile = true;
                    continue;
                }
                Change::Removed(id) => {
                    patch.remove.push(self.text_keys.remove(&id).unwrap_or(id));
                    continue;
                }
                Change::Inserted(id) => (id, true),
                Change::Updated(id) => (id, false),
            };
            let entry = self.fold.iter().rev().find(|entry| match entry {
                TranscriptEntry::Tool(row) => row.id == id,
                TranscriptEntry::Step(step) => step.id == id,
                _ => false,
            });
            if let Some(TranscriptEntry::Step(_)) = entry {
                if inserted {
                    let source = seq.map_or_else(|| id.clone(), |seq| seq.to_string());
                    self.text_keys
                        .insert(id.clone(), format!("text/{}/{source}", self.id));
                }
            }
            if let Some(row) = entry.and_then(|entry| self.row(entry)) {
                if inserted {
                    patch.insert(row);
                } else {
                    patch.update(row);
                }
            }
        }
    }
}

fn append_replay(
    run_id: &str,
    replay: &mut HashMap<String, LiveRun>,
    rows: &mut Vec<Row>,
    diffs: &mut HashMap<String, RowBody>,
) {
    let Some(run) = replay.remove(run_id) else {
        return;
    };
    rows.extend(run.rows());
    for entry in run.fold.iter() {
        if let TranscriptEntry::Tool(row) = entry {
            if matches!(row.body, RowBody::FileChanges(_)) {
                diffs.insert(row.id.clone(), row.body.clone());
            }
        }
    }
}

fn encode(value: impl Serialize) -> Value {
    serde_json::to_value(value).expect("chat view serialization is infallible")
}

#[cfg(test)]
#[path = "chat_panel_tests.rs"]
mod tests;
