//! Carry the conversation's file-operation ledger across context compaction.
//!
//! Which files this conversation has already read, and which it has already
//! changed, is a *fact* — and after a compaction it exists nowhere the model can
//! reach. The `ToolCall` blocks that named the paths are drained into the
//! summary, and the results that carried the bytes were already stubbed by
//! [`file_op_supersede`](crate::context::budget::cheap_passes::file_op_supersede)
//! one step earlier. What survives is prose, and prose is where "I already
//! rewrote `src/store.rs`" turns into "the assistant worked on the store" —
//! after which the model re-reads files it wrote and re-derives edits it made.
//!
//! Port of pi's cumulative file tracking (`compaction/utils.ts`
//! `computeFileLists` / `formatFileOperations`, appended to every summary as
//! `<read-files>` / `<modified-files>`), with three deliberate divergences:
//!
//! 1. **Carried, not summarized.** pi appends the lists to the summary *text*,
//!    which means a summarizer failure (Aleph's deterministic-truncation
//!    fallback) loses them. Here the ledger is its own message produced by the
//!    same [`splice_preserved`](super::compactor) shape that already carries the
//!    user's verbatim turns and the execution list, so it survives every drain
//!    path including the zero-LLM ones.
//! 2. **Failed calls do not count.** pi records the path off the tool *call*.
//!    A write whose result came back `is_error` never changed the file, and a
//!    read that failed produced no bytes; claiming either is a fact the model
//!    would act on. The success gate is
//!    [`file_ops::successful_result_ids`](crate::context::file_ops::successful_result_ids)
//!    — the same predicate the supersede pass uses to decide what may
//!    invalidate an earlier result.
//! 3. **Bounded.** The list is spliced in *after* the budget arithmetic is done,
//!    at the one moment the window was already over budget, so an unbounded
//!    ledger would be paid unaccounted on every request until the next
//!    compaction (the same argument [`super::plan_carry`] bounds its render on).
//!    Newest-touched paths win the cap and the elision is stated out loud.
//!
//! Pure — no I/O, no session key, no store lookup. Everything needed is in the
//! messages being drained.

use crate::context::file_ops::{self, FileOpKind};
use crate::providers::message::{ContentBlock, UnifiedMessage};

/// Stable sentinel opening a carried-over file ledger. Recognised on the way
/// back in so a later compaction pass re-carries a ledger whose originating
/// tool calls were already drained by an earlier one — this is what makes the
/// ledger *cumulative* across compaction cycles rather than a view of the last
/// window only.
const LEDGER_MARKER: &str = "[Files touched, preserved across context compaction]";

/// Line prefix for a modified path in the rendered ledger.
const MODIFIED_PREFIX: &str = "M ";

/// Line prefix for a read-only path in the rendered ledger.
const READ_PREFIX: &str = "R ";

/// Ceiling on how many paths the ledger names. Modified paths are allocated
/// first — "I already changed this" is the more actionable half, and the more
/// expensive one to get wrong.
const MAX_LISTED_PATHS: usize = 40;

/// A path longer than this is rendered head-elided. Generated paths (build
/// artifacts, deep node_modules trees) can be hundreds of characters, and forty
/// of them would defeat the point of a bounded carrier.
const MAX_PATH_CHARS: usize = 120;

/// Read and modified paths, each newest-touched first and deduplicated.
///
/// Recency order rather than lexicographic: the cap has to drop something, and
/// the least useful thing to drop is what the model touched last. Deterministic
/// all the same — the window is a pure function of the append-only session log,
/// so the same rebuild yields the same order.
#[derive(Default, Debug, PartialEq, Eq)]
struct Ledger {
    modified: Vec<String>,
    read: Vec<String>,
}

impl Ledger {
    fn is_empty(&self) -> bool {
        self.modified.is_empty() && self.read.is_empty()
    }

    /// Record `path` under `kind`, newest-first, without duplicating it.
    /// Called in reverse-chronological order, so the first sighting wins.
    fn push(&mut self, kind: FileOpKind, path: String) {
        if self.modified.contains(&path) {
            return;
        }
        if kind.is_mutating() {
            // A path can be seen as read first (later in the conversation) and
            // as modified afterwards (earlier in the conversation). Modified
            // outranks read — pi's `computeFileLists` filters the read list by
            // the modified set for the same reason.
            self.read.retain(|p| p != &path);
            self.modified.push(path);
        } else if !self.read.contains(&path) {
            self.read.push(path);
        }
    }
}

/// Render the file-ledger message for a compaction window, or `None` when the
/// window records no successful file operation and carries no prior ledger —
/// the common case for a conversation that never touched the filesystem.
pub(crate) fn file_carry_message(window: &[UnifiedMessage]) -> Option<UnifiedMessage> {
    let ledger = collect(window);
    if ledger.is_empty() {
        return None;
    }
    Some(UnifiedMessage::User {
        content: vec![ContentBlock::Text {
            text: render(&ledger),
            cache_control: None,
        }],
    })
}

/// Build the cumulative ledger for `window`: the successful file ops it
/// contains, newest first, followed by whatever a previous pass already
/// recorded.
fn collect(window: &[UnifiedMessage]) -> Ledger {
    let successful = file_ops::successful_result_ids(window);
    let mut ledger = Ledger::default();

    for op in file_ops::index_file_ops(window).into_iter().rev() {
        if successful.contains(&op.call_id) {
            ledger.push(op.kind, op.path);
        }
    }

    // Prior ledgers rank *after* this window's own ops: their paths are older by
    // construction, so they are the right thing to lose to the cap.
    if let Some(prior) = latest_prior_ledger(window) {
        for path in prior.modified {
            ledger.push(FileOpKind::Write, path);
        }
        for path in prior.read {
            ledger.push(FileOpKind::Read, path);
        }
    }

    ledger
}

/// The newest ledger this module itself emitted inside `window`, parsed back.
fn latest_prior_ledger(window: &[UnifiedMessage]) -> Option<Ledger> {
    window.iter().rev().find_map(|msg| match msg {
        UnifiedMessage::User { content } => content.iter().find_map(|block| match block {
            ContentBlock::Text { text, .. } if text.contains(LEDGER_MARKER) => Some(parse(text)),
            _ => None,
        }),
        _ => None,
    })
}

/// Read back the line format [`render`] emits. Lines that are neither prefix
/// are ignored, so the surrounding framing (and the elision note) round-trips
/// harmlessly.
fn parse(text: &str) -> Ledger {
    let mut ledger = Ledger::default();
    for line in text.lines() {
        let line = line.trim();
        if let Some(path) = line.strip_prefix(MODIFIED_PREFIX) {
            ledger.push(FileOpKind::Write, path.trim().to_string());
        } else if let Some(path) = line.strip_prefix(READ_PREFIX) {
            ledger.push(FileOpKind::Read, path.trim().to_string());
        }
    }
    ledger
}

/// Render the ledger, capped at [`MAX_LISTED_PATHS`] with the elision stated.
///
/// Wrapped in `<system-reminder>` for the same reason [`super::plan_carry`] is:
/// it rides the list in the `User` role but the user did not write it, and the
/// classifier every consumer shares
/// ([`is_synthetic_reminder`](crate::thinker::nudges::is_synthetic_reminder))
/// is what keeps it out of `<conversation_focus>` and out of verbatim user-turn
/// preservation.
fn render(ledger: &Ledger) -> String {
    let modified_shown = ledger.modified.len().min(MAX_LISTED_PATHS);
    let read_shown = ledger.read.len().min(MAX_LISTED_PATHS - modified_shown);
    let elided = (ledger.modified.len() - modified_shown) + (ledger.read.len() - read_shown);

    let mut lines = Vec::with_capacity(modified_shown + read_shown + 1);
    for path in ledger.modified.iter().take(modified_shown) {
        lines.push(format!("{MODIFIED_PREFIX}{}", elide_path(path)));
    }
    for path in ledger.read.iter().take(read_shown) {
        lines.push(format!("{READ_PREFIX}{}", elide_path(path)));
    }
    if elided > 0 {
        lines.push(format!("(+{elided} more file(s) not listed)"));
    }

    format!(
        "<system-reminder>\nReference data, not user input.\n{LEDGER_MARKER}\n\
         `M` = already modified by this conversation, `R` = already read.\n{}\n\
         Re-read a file only when you need content you no longer hold.\n\
         </system-reminder>",
        lines.join("\n")
    )
}

/// Head-elide an over-long path on a char boundary (P7 UTF-8 safety), keeping
/// the tail — the file name is the identifying half, the ancestor directories
/// are not.
fn elide_path(path: &str) -> String {
    let count = path.chars().count();
    if count <= MAX_PATH_CHARS {
        return path.to_string();
    }
    let tail: String = path
        .chars()
        .skip(count - MAX_PATH_CHARS)
        .collect::<String>();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(id: &str, name: &str, path: &str) -> UnifiedMessage {
        UnifiedMessage::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: id.into(),
                name: name.into(),
                arguments: json!({ "path": path }),
                thought_signature: None,
            }],
        }
    }

    fn ok(id: &str, name: &str) -> UnifiedMessage {
        UnifiedMessage::tool_result(id, name, "done", false)
    }

    fn failed(id: &str, name: &str) -> UnifiedMessage {
        UnifiedMessage::tool_result(id, name, "boom", true)
    }

    fn text_of(msg: &UnifiedMessage) -> String {
        msg.text_content()
    }

    #[test]
    fn a_window_with_no_file_ops_carries_nothing() {
        let window = vec![
            UnifiedMessage::user("hello"),
            UnifiedMessage::assistant("hi"),
        ];
        assert!(
            file_carry_message(&window).is_none(),
            "calm windows pay nothing"
        );
    }

    #[test]
    fn modified_outranks_read_for_the_same_path() {
        // Read then edited: the model needs to know it already changed the file,
        // not that it once looked at it.
        let window = vec![
            call("c1", "file_read", "src/a.rs"),
            ok("c1", "file_read"),
            call("c2", "file_edit", "src/a.rs"),
            ok("c2", "file_edit"),
        ];
        let text = text_of(&file_carry_message(&window).expect("a ledger"));
        assert!(text.contains("M src/a.rs"), "got: {text}");
        assert!(!text.contains("R src/a.rs"), "got: {text}");
    }

    #[test]
    fn a_failed_call_is_not_a_fact() {
        // The write never landed, so claiming the file was modified would send
        // the model on to the next step believing work it does not have.
        let window = vec![
            call("c1", "file_write", "src/a.rs"),
            failed("c1", "file_write"),
            call("c2", "file_read", "src/b.rs"),
            ok("c2", "file_read"),
        ];
        let text = text_of(&file_carry_message(&window).expect("a ledger"));
        assert!(
            !text.contains("src/a.rs"),
            "failed write must not be listed: {text}"
        );
        assert!(text.contains("R src/b.rs"), "got: {text}");
    }

    #[test]
    fn a_call_whose_result_never_arrived_is_not_a_fact_either() {
        // No `ToolResult` at all (interrupted turn) is the same epistemic state
        // as a failure: we do not know the op happened.
        let window = vec![call("c1", "file_write", "src/a.rs")];
        assert!(file_carry_message(&window).is_none());
    }

    #[test]
    fn the_ledger_accumulates_across_compaction_cycles() {
        // Second pass: the original tool calls are gone (drained by the first
        // compaction), only the carry this module emitted survives — plus one
        // new op. Both must appear.
        let first = vec![
            call("c1", "file_edit", "src/old.rs"),
            ok("c1", "file_edit"),
            call("c2", "file_read", "src/seen.rs"),
            ok("c2", "file_read"),
        ];
        let carried = file_carry_message(&first).expect("a ledger");

        let second = vec![
            carried,
            call("c3", "file_write", "src/new.rs"),
            ok("c3", "file_write"),
        ];
        let text = text_of(&file_carry_message(&second).expect("a ledger"));
        assert!(text.contains("M src/new.rs"), "got: {text}");
        assert!(
            text.contains("M src/old.rs"),
            "prior modified must survive: {text}"
        );
        assert!(
            text.contains("R src/seen.rs"),
            "prior read must survive: {text}"
        );
    }

    #[test]
    fn a_path_modified_in_a_prior_cycle_stays_modified_when_only_read_later() {
        let first = vec![call("c1", "file_write", "src/a.rs"), ok("c1", "file_write")];
        let carried = file_carry_message(&first).expect("a ledger");
        let second = vec![
            carried,
            call("c2", "file_read", "src/a.rs"),
            ok("c2", "file_read"),
        ];
        let text = text_of(&file_carry_message(&second).expect("a ledger"));
        assert!(text.contains("M src/a.rs"), "got: {text}");
        assert!(
            !text.contains("R src/a.rs"),
            "must not be demoted to read: {text}"
        );
    }

    #[test]
    fn the_list_is_capped_and_says_so() {
        let mut window = Vec::new();
        for i in 0..(MAX_LISTED_PATHS + 12) {
            let id = format!("c{i}");
            window.push(call(&id, "file_read", &format!("src/f{i}.rs")));
            window.push(ok(&id, "file_read"));
        }
        let text = text_of(&file_carry_message(&window).expect("a ledger"));
        let listed = text.lines().filter(|l| l.starts_with(READ_PREFIX)).count();
        assert_eq!(listed, MAX_LISTED_PATHS, "cap must bind");
        assert!(
            text.contains("(+12 more file(s) not listed)"),
            "got: {text}"
        );
    }

    #[test]
    fn modified_paths_win_the_cap_over_read_paths() {
        let mut window = Vec::new();
        for i in 0..MAX_LISTED_PATHS {
            let id = format!("r{i}");
            window.push(call(&id, "file_read", &format!("read/{i}.rs")));
            window.push(ok(&id, "file_read"));
        }
        for i in 0..5 {
            let id = format!("w{i}");
            window.push(call(&id, "file_write", &format!("mod/{i}.rs")));
            window.push(ok(&id, "file_write"));
        }
        let text = text_of(&file_carry_message(&window).expect("a ledger"));
        for i in 0..5 {
            assert!(text.contains(&format!("M mod/{i}.rs")), "got: {text}");
        }
        assert_eq!(
            text.lines().filter(|l| l.starts_with(READ_PREFIX)).count(),
            MAX_LISTED_PATHS - 5
        );
    }

    #[test]
    fn the_carry_is_classified_as_scaffolding_by_the_shared_predicate() {
        // Load-bearing: this message rides in the `User` role. If the shared
        // classifier did not recognise it, verbatim user-turn preservation
        // would re-attach it as intent and `latest_user_task` would anchor
        // `<conversation_focus>` to a file list.
        let window = vec![call("c1", "file_read", "src/a.rs"), ok("c1", "file_read")];
        let text = text_of(&file_carry_message(&window).expect("a ledger"));
        assert!(crate::thinker::nudges::is_synthetic_reminder(&text));
        assert!(super::super::preserve::is_synthetic_scaffold(&text));
    }

    #[test]
    fn an_over_long_path_keeps_its_tail() {
        let long = format!("{}/deep/file.rs", "a".repeat(400));
        let window = vec![call("c1", "file_read", &long), ok("c1", "file_read")];
        let text = text_of(&file_carry_message(&window).expect("a ledger"));
        assert!(
            text.contains("deep/file.rs"),
            "the identifying half survives"
        );
        assert!(text.contains('…'));
        assert!(
            text.lines()
                .all(|l| l.chars().count() <= MAX_PATH_CHARS + 8),
            "no line may blow the bound: {text}"
        );
    }

    #[test]
    fn rendering_is_stable_across_rebuilds() {
        // The carry is spliced into a prompt that the provider caches by prefix;
        // a set-iteration-order render would re-key it every turn for free.
        let window = vec![
            call("c1", "file_read", "src/b.rs"),
            ok("c1", "file_read"),
            call("c2", "file_write", "src/a.rs"),
            ok("c2", "file_write"),
            call("c3", "file_read", "src/c.rs"),
            ok("c3", "file_read"),
        ];
        let a = text_of(&file_carry_message(&window).expect("a ledger"));
        let b = text_of(&file_carry_message(&window).expect("a ledger"));
        assert_eq!(a, b);
    }
}
