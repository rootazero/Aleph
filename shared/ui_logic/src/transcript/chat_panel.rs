//! NSH Chat's transport-neutral state and render patches.

use std::collections::{HashMap, HashSet, VecDeque};

use aleph_protocol::chat_transcript::{ChatToolChange, ChatToolDelta, ChatTranscriptDetails};
use aleph_protocol::StreamEvent;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Change, RowBody, RowStatus, StepStatus, Transcript, TranscriptEntry};

const EARLY_FRAMES: usize = 64;
const EARLY_BYTES: usize = 256 * 1024;
const EARLY_AGE_MS: u64 = 10_000;
const UNCONFIRMED_RUN_LIMIT: usize = 8;

#[derive(Default)]
pub struct ChatState {
    session: String,
    live: Option<LiveRun>,
    sealed: VecDeque<LiveRun>,
    dropped_unconfirmed: bool,
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
    can_resume: bool,
    detached: bool,
    prompt: Option<Prompt>,
}

struct Prompt {
    key: String,
    text: String,
}

struct PendingSend {
    prompt: Prompt,
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
    #[serde(default)]
    live_status_stale: bool,
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
    tool_call_id: Option<String>,
    change: Option<ChatToolChange>,
    error: Option<String>,
    #[serde(default)]
    is_final: bool,
    #[serde(default)]
    is_intermediate: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    insert_after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    insert_before: Option<String>,
    reconcile: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_run: Option<String>,
    active_run: Option<String>,
    missing_prefix: bool,
    unconfirmed: bool,
    dropped_unconfirmed: bool,
    can_resume: bool,
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
        if self.live.is_some() || self.pending.is_some() {
            return encode(patch);
        }
        self.generation += 1;
        let pending = PendingSend {
            prompt: Prompt {
                key: format!("local/{}", self.generation),
                text,
            },
            started_ms: now_ms,
            held: Vec::new(),
            bytes: 0,
            incomplete: false,
        };
        patch.insert(pending.prompt.row());
        self.pending = Some(pending);
        encode(patch)
    }

    pub fn resolve_send(&mut self, session: String, run: Option<String>, now_ms: u64) -> Value {
        let mut patch = self.patch();
        let Some(mut pending) = self.pending.take() else {
            return encode(patch);
        };
        let Some(run_id) = run else {
            patch.remove.push(pending.prompt.key);
            return encode(patch);
        };
        self.session = session;
        let held = std::mem::take(&mut pending.held);
        let expired = now_ms.saturating_sub(pending.started_ms) > EARLY_AGE_MS;
        let missing = pending.incomplete || expired;
        let mut run = LiveRun::new(run_id.clone(), now_ms, missing);
        run.prompt = Some(pending.prompt);
        self.live = Some(run);
        if !expired {
            for frame in held.into_iter().filter(|frame| frame.data.run_id == run_id) {
                self.fold_live_frame(frame, now_ms, &mut patch);
            }
        }
        patch.reconcile |= missing;
        self.finish_patch(&mut patch);
        encode(patch)
    }

    pub fn accept(&mut self, event: Value) -> Value {
        let now_ms = event.get("nowMs").and_then(Value::as_u64).unwrap_or(0);
        let bytes = if self.pending.is_some() {
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
            self.fold_live_frame(frame, now_ms, &mut patch);
        } else if let Some(run) = self
            .sealed
            .iter_mut()
            .find(|run| run.detached && run.id == frame.data.run_id)
        {
            let anchor = run.last_row_key().map(str::to_owned);
            if Self::fold_frame(run, frame, now_ms, &mut patch) {
                run.detached = false;
            }
            if !patch.append_order.is_empty() {
                patch.insert_after = anchor.or_else(|| Some(run.preview_key()));
            }
        } else if let Some(pending) = &mut self.pending {
            // Keep complete frames only and never retain unrelated data after the send reply.
            if now_ms.saturating_sub(pending.started_ms) > EARLY_AGE_MS {
                pending.held.clear();
                pending.bytes = 0;
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
            run.can_resume = false;
        }
        if let Some(pending) = &mut self.pending {
            pending.held.clear();
            pending.bytes = 0;
            pending.incomplete = true;
        }
        encode(self.patch())
    }

    /// Allow composing without losing the interrupted run's live subscription.
    pub fn resume_composing(&mut self) -> Value {
        let mut patch = self.patch();
        if self.live.as_ref().is_some_and(|run| run.can_resume) {
            self.retain_live_preview(&mut patch, true);
            self.finish_patch(&mut patch);
        }
        encode(patch)
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
        self.sealed.retain(|run| {
            !assistant_runs.contains(run.id.as_str())
                || (run.detached
                    && (history.live_status_stale
                        || history.active_run.as_deref() == Some(&run.id)))
        });
        // Send acknowledgement can precede execution registration. A missing
        // active slot alone is not evidence that the run ended.
        if self.live.as_ref().is_some_and(|run| {
            !history.live_status_stale
                && assistant_runs.contains(run.id.as_str())
                && history.active_run.as_deref() != Some(&run.id)
        }) {
            self.last_ended = self.live.take().map(|run| run.id);
        }
        if !history.live_status_stale && self.live.is_none() && self.pending.is_none() {
            if let Some(id) = &history.active_run {
                match self.sealed.iter().position(|run| &run.id == id) {
                    Some(index) if self.sealed[index].detached => {
                        self.live = self.sealed.remove(index);
                        if let Some(run) = &mut self.live {
                            run.detached = false;
                        }
                    }
                    None if Some(id) != self.last_ended.as_ref()
                        && !assistant_runs.contains(id.as_str()) =>
                    {
                        self.live = Some(LiveRun::new(id.clone(), 0, true));
                    }
                    _ => {}
                }
            }
        }
        if let Some(run) = &mut self.live {
            run.can_resume =
                !history.live_status_stale && run.missing_prefix && history.active_run.is_none();
        }
        let user_runs: HashSet<_> = history
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .filter_map(|message| message.run_id.as_deref())
            .collect();
        for run in self.live.iter_mut().chain(&mut self.sealed) {
            if user_runs.contains(run.id.as_str()) {
                run.prompt = None;
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
            if let Some(run) = message
                .run_id
                .as_deref()
                .filter(|_| matches!(message.role.as_str(), "user" | "assistant"))
            {
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
            // Only the dedicated replay projection may render stored tool data.
            if !matches!(message.role.as_str(), "user" | "assistant") {
                continue;
            }
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
        for run in &self.sealed {
            rows.push(run.preview_label());
            rows.extend(run.rows());
        }
        if let Some(pending) = &self.pending {
            rows.push(pending.prompt.row());
        }
        if let Some(run) = &self.live {
            rows.extend(run.rows());
        }
        json!({"rows": rows, "activeRun": self.live.as_ref().map(|run| &run.id),
            "missingPrefix": self.has_missing_prefix(), "unconfirmed": !self.sealed.is_empty(),
            "unconfirmedRuns": self.sealed.iter().map(|run| &run.id).collect::<Vec<_>>(),
            "droppedUnconfirmed": self.dropped_unconfirmed, "limited": details.limited,
            "canResume": self.live.as_ref().is_some_and(|run| run.can_resume)})
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
        patch.missing_prefix = self.has_missing_prefix();
        patch.unconfirmed = !self.sealed.is_empty();
        patch.dropped_unconfirmed = self.dropped_unconfirmed;
        patch.can_resume = self.live.as_ref().is_some_and(|run| run.can_resume);
    }

    fn has_missing_prefix(&self) -> bool {
        self.live
            .iter()
            .chain(&self.sealed)
            .any(|run| run.missing_prefix)
    }

    fn fold_live_frame(&mut self, frame: ChatFrame, now_ms: u64, patch: &mut Patch) {
        if let Some(run) = &mut self.live {
            if Self::fold_frame(run, frame, now_ms, patch) {
                self.retain_live_preview(patch, false);
            }
        }
    }

    fn fold_frame(run: &mut LiveRun, frame: ChatFrame, now_ms: u64, patch: &mut Patch) -> bool {
        let data = frame.data;
        if data
            .seq
            .is_some_and(|seq| run.last_seq.is_some_and(|last| last >= seq))
        {
            return false;
        }
        if data.seq.is_some() {
            run.last_seq = data.seq;
        }
        run.can_resume = false;
        // A final, non-intermediate chunk is the last frame on graceful
        // hook stops, which bypass the normal event drain.
        let terminal = match frame.topic.as_str() {
            "stream.response_chunk" => data.is_final && !data.is_intermediate,
            "stream.run_complete" | "stream.run_error" => true,
            _ => false,
        };
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
            "stream.chat_tool" => data.tool_call_id.zip(data.change).map_or_else(
                Vec::new,
                |(tool_call_id, change)| {
                    run.fold_tool(
                        &ChatToolDelta {
                            tool_call_id,
                            change,
                        },
                        now_ms,
                    )
                },
            ),
            "stream.run_error" => {
                patch.error = Some(data.error.unwrap_or_else(|| "unknown error".into()));
                Vec::new()
            }
            _ => Vec::new(),
        };
        run.project_changes(changes, data.seq, patch);
        if terminal {
            let changes = run.fold.seal_chat_run(&run.id, now_ms);
            run.project_changes(changes, data.seq, patch);
            patch.reconcile = true;
            patch.terminal_run = Some(run.id.clone());
        }
        terminal
    }

    fn retain_live_preview(&mut self, patch: &mut Patch, detached: bool) {
        let Some(mut run) = self.live.take() else {
            return;
        };
        run.detached = detached;
        run.can_resume = false;
        if detached {
            patch.insert_before = run.rows().next().map(|first| first.key().to_owned());
            patch.insert(run.preview_label());
        } else {
            self.last_ended = Some(run.id.clone());
        }
        if self.sealed.len() == UNCONFIRMED_RUN_LIMIT {
            if let Some(previous) = self.sealed.pop_front() {
                self.dropped_unconfirmed = true;
                patch.remove.push(previous.preview_key());
                patch
                    .remove
                    .extend(previous.rows().map(|row| row.key().to_owned()));
            }
        }
        self.sealed.push_back(run);
    }
}

impl Prompt {
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
            can_resume: false,
            detached: false,
            prompt: None,
        }
    }

    fn preview_key(&self) -> String {
        format!("unconfirmed/{}", self.id)
    }

    fn preview_label(&self) -> Row {
        Row::Text {
            key: self.preview_key(),
            role: "notice".into(),
            text: "Unconfirmed turn".into(),
            streaming: false,
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

    fn last_row_key(&self) -> Option<&str> {
        self.fold
            .iter()
            .rev()
            .find_map(|entry| match entry {
                TranscriptEntry::Tool(row) => Some(row.id.as_str()),
                TranscriptEntry::Step(step) if step.text.is_some() => {
                    self.text_keys.get(&step.id).map(String::as_str)
                }
                _ => None,
            })
            .or_else(|| self.prompt.as_ref().map(|prompt| prompt.key.as_str()))
    }

    fn rows(&self) -> impl Iterator<Item = Row> + '_ {
        self.prompt
            .iter()
            .map(Prompt::row)
            .chain(self.fold.iter().filter_map(|entry| self.row(entry)))
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
