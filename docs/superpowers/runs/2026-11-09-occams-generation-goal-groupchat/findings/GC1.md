# Batch GC1 Static Occam's Razor Review Report
**Scope**: `src/group_chat` (8 files, 3796 LOC)  
**Worktree**: `/home/zou/data/workspace/Aleph-wt-occams-gen-goal-gc`  
**Review Standards**: `/rust-occams-razor`, Non-Negotiable Invariants 1–7

---

## Executive Summary & Findings Table

| Finding ID | File & Line | Category | Severity | Summary |
|------------|-------------|----------|----------|---------|
| **GC1-01** | `src/group_chat/channel.rs:41-58` | Control Flow / Robustness | HIGH | Nested/repetitive `if/else if` strips prefixes and drops commands when separated by `\t` |
| **GC1-02** | `src/group_chat/channel.rs:135-143` | Defensive Parsing | MEDIUM | `--preset` fails to guard against swallowing subsequent `--flags` unlike `--role` and `--topic` |
| **GC1-03** | `src/group_chat/coordinator.rs:197-222` | Parser Robustness / Fragility | HIGH | `strip_markdown_fences` case-sensitive `"json"` fails on ```` ```JSON ```` and skips brace fallback |
| **GC1-04** | `src/group_chat/coordinator.rs:189-195` | Redundant Allocation | LOW | `json_safe_field` allocates 2 heap `String`s and runs 2 scans per field instead of 1 pass |
| **GC1-05** | `src/group_chat/coordinator.rs:39,52,69,73` | Redundant Allocation | LOW | `prompt.push_str(&format!(...))` allocates 15–20 temporary `String`s per round |
| **GC1-06** | `src/group_chat/session.rs:241-267` | Redundant Allocation & Structures | MEDIUM | `build_history_text_windowed` creates `BTreeSet` and `Vec` on sorted `self.history` |
| **GC1-07** | `src/group_chat/executor.rs:159-162` | Redundant Allocation & Clones | MEDIUM | `persist_turn` clones `Arc<StateDatabase>` and creates 2 owned `String`s from existing `&str` |
| **GC1-08** | `src/group_chat/executor.rs:366-370` | Ordering / Priority Bug | HIGH | `truncate` executes before `sort_by_key(|r| r.order)`, prematurely discarding higher-priority respondents |
| **GC1-09** | `src/group_chat/executor.rs:364-365` | Deduplication Bug | MEDIUM | Respondent deduplication is case-sensitive while participant lookup is case-insensitive |
| **GC1-10** | `src/group_chat/executor.rs:287,430` | Redundant Arc Clones | LOW | `session.cancel_token.clone()` before every LLM call when `select!` only requires `&self` |
| **GC1-11** | `src/group_chat/executor.rs:504-568` | Redundant Allocation & Clones | MEDIUM | Large LLM response strings and persona metadata are cloned 3 times instead of moved |
| **GC1-12** | `src/group_chat/orchestrator.rs:137-146` | Validation Bug | MEDIUM | `create_session` checks duplicate persona IDs case-sensitively, allowing case collisions that shadow personas in executor |
| **GC1-13** | `src/group_chat/protocol.rs:84-118` | Validation Gap | LOW | `Persona::validate` allows whitespace-only `id`, `name`, or `system_prompt` |
| **GC1-14** | `src/group_chat/persona.rs:23-36` | Redundant Allocation | LOW | `from_configs` lacks capacity reservation and clones `cfg.id` twice per entry |

---

## Detailed Findings & Proposed Surgical Fixes

### GC1-01: Command Parser Whitespace Sensitivity and Repetitive Prefix Stripping
- **File & Line**: `src/group_chat/channel.rs:41-58`
- **Issue & Accidental Complexity**:
  `parse_group_chat_command` uses a 4-level cascaded `if / else if` chain checking `after == "subcmd" || after.starts_with("subcmd ")` and then re-calls `after.strip_prefix("subcmd")?.trim()` inside each branch.
- **Concrete Failure Mode**:
  - *Trigger*: User enters a valid `/groupchat` command where the subcommand is followed by a tab `\t` (common in shell / automated scripts) or multiple spaces / newline, e.g. `/groupchat start\t--preset p1`.
  - *Code Path*: `after` is `"start\t--preset p1"`. `after == "start"` evaluates to `false`. `after.starts_with("start ")` evaluates to `false` because the separator character is `\t`, not `' '`.
  - *Wrong Outcome*: All 4 branches fail to match and the parser falls through to `None`, silently ignoring a valid user command.
- **Proposed Minimal Fix**:
  Split once on whitespace (`char::is_whitespace`) and match on the verb:
  ```rust
  let (verb, args) = match after.split_once(char::is_whitespace) {
      Some((v, a)) => (v, a.trim()),
      None => (after, ""),
  };

  match verb {
      "start" => parse_start_command(args),
      "continue" => parse_continue_command(args),
      "mention" => parse_mention_command(args),
      "end" => Some(GroupChatRequest::End {
          session_id: args.to_string(),
      }),
      _ => None,
  }
  ```

### GC1-02: Missing Adjacent Flag Check on `--preset` Parameter
- **File & Line**: `src/group_chat/channel.rs:135-143`
- **Issue**:
  `--role` (lines 149-151) and `--topic` (lines 158-160) have explicit defensive checks (`if val.starts_with("--") { return None; }`) to avoid consuming the next flag as an argument value. `--preset` lacks this check.
- **Concrete Failure Mode**:
  - *Trigger*: A user types `/groupchat start --preset --topic "Architecture" Hello`.
  - *Code Path*: `--preset` consumes `val = iter.next()?`, which is `"--topic"`. `val.split(',')` produces `"--topic"`, which is pushed into `personas` as `PersonaSource::Preset("--topic")`.
  - *Wrong Outcome*: The `--topic` flag and value are silently swallowed into a bogus persona ID. Downstream session creation fails with `GroupChatError::PersonaNotFound("--topic")` instead of rejecting the malformed command at parse time.
- **Proposed Minimal Fix**:
  Add `if val.starts_with("--") { return None; }` to `--preset`.

### GC1-03: `strip_markdown_fences` Case-Sensitive `"json"` Tag Breakage
- **File & Line**: `src/group_chat/coordinator.rs:197-222`
- **Issue**:
  `strip_markdown_fences` checks only `after_fence.strip_prefix("json")`.
- **Concrete Failure Mode**:
  - *Trigger*: Coordinator LLM outputs JSON enclosed in markdown fences with uppercase language tag like ```` ```JSON\n{"respondents":[]}\n``` ```` or ```` ```json-object ````.
  - *Code Path*: `s.find("```")` succeeds. `after_fence.strip_prefix("json")` returns `None` due to case sensitivity. Slicing returns `"JSON\n{\"respondents\":[]}"`.
  - *Wrong Outcome*: `serde_json::from_str` fails with `expected value at line 1 column 1`. The fallback `{` .. `}` brace search is bypassed because the function prematurely returned.
- **Proposed Minimal Fix**:
  Strip the entire fence-declaration line up to the first newline `\n`.

### GC1-04: Double-Pass Allocation in `json_safe_field`
- **File & Line**: `src/group_chat/coordinator.rs:189-195`
- **Issue**:
  `s.chars().filter(|c| !c.is_control()).collect::<String>().replace('"', "\u{201D}")` performs two passes and allocates two heap `String`s.
- **Proposed Minimal Fix**:
  Single pass loop over characters into preallocated string.

### GC1-05: Redundant Intermediate `format!` Allocations in Prompt Builders
- **File & Line**: `src/group_chat/coordinator.rs:39, 52, 69, 73, 155, 159, 169, 173`
- **Issue**:
  Repeated `prompt.push_str(&format!(...))` allocates temporary formatting strings before immediately copying them into `prompt`.
- **Proposed Minimal Fix**:
  Use `std::fmt::Write as _;` and `writeln!(prompt, ...)`.

### GC1-06: Unnecessary `BTreeSet` & `Vec` in `build_history_text_windowed`
- **File & Line**: `src/group_chat/session.rs:241-267`
- **Issue**:
  Iterates over `self.history`, allocating a `BTreeSet<u32>` and a `Vec<&GroupChatTurn>`, despite `self.history` being strictly monotonic by `round`.
- **Proposed Minimal Fix**:
  Use `partition_point` (O(log N)) and slice references without extra collections.

### GC1-07: Redundant Arc Clones and String Allocations in `persist_turn`
- **File & Line**: `src/group_chat/executor.rs:159-162`
- **Issue**:
  `persist_turn` performs `self.db.clone()`, `session_id.to_string()`, and `content.to_string()` before calling `insert_group_chat_turn`.
- **Proposed Minimal Fix**:
  Pass `&str` references directly to `insert_group_chat_turn`.

### GC1-08: Inverted `truncate` / `sort_by_key` Discarding Priority Respondents
- **File & Line**: `src/group_chat/executor.rs:366-370`
- **Issue**:
  `plan.respondents.truncate(session.participants.len())` runs *before* `plan.respondents.sort_by_key(|r| r.order)`.
- **Concrete Failure Mode**:
  - *Trigger*: The coordinator returns more respondents than participants, where explicit `order` values do not match array declaration order (e.g. `[{"persona_id": "c", "order": 5}, {"persona_id": "a", "order": 1}, {"persona_id": "b", "order": 2}]` for a 2-persona session).
  - *Wrong Outcome*: Persona `b` (scheduled to speak 2nd) was dropped, while `c` (scheduled to speak 5th) was retained and executed, violating the coordinator's intended priority.
- **Proposed Minimal Fix**:
  Swap the two statements: sort first, then truncate.

### GC1-09: Case-Sensitive Deduplication vs Case-Insensitive Participant Lookup
- **File & Line**: `src/group_chat/executor.rs:364-365`
- **Issue**:
  Respondent deduplication uses case-sensitive `seen.insert(r.persona_id.clone())`, while persona lookup uses `eq_ignore_ascii_case`.
- **Proposed Minimal Fix**:
  `seen.insert(r.persona_id.to_ascii_lowercase())`.

### GC1-10: Redundant `CancellationToken::clone()` in Select Loops
- **File & Line**: `src/group_chat/executor.rs:287, 430`
- **Issue**:
  `let cancel = session.cancel_token.clone();` is executed before the coordinator call and on every iteration of the persona invocation loop.
- **Proposed Minimal Fix**:
  Select directly on `_ = session.cancel_token.cancelled() =>`.

### GC1-11: Triple Cloning of LLM Responses Across Staging and Emission
- **File & Line**: `src/group_chat/executor.rs:504-568`
- **Issue**:
  `p.content` is cloned for `session.add_turn`, cloned for `staged_turns`, and cloned a 3rd time for `messages.push`.
- **Proposed Minimal Fix**:
  Consume `prepared.into_iter()` by move.

### GC1-12: Case-Sensitive Duplicate Check in `create_session` Permitting Shadowing
- **File & Line**: `src/group_chat/orchestrator.rs:137-146`
- **Issue**:
  `create_session` checks duplicate persona IDs with case-sensitive hashing while `executor` matches case-insensitively.
- **Proposed Minimal Fix**:
  Use `seen_ids.insert(p.id.to_ascii_lowercase())`.

### GC1-13: Missing Whitespace-Only Validation in `Persona::validate`
- **File & Line**: `src/group_chat/protocol.rs:84-118`
- **Issue**:
  `validate()` checks `is_empty()` but allows whitespace-only strings for `id`, `name`, and `system_prompt`.
- **Proposed Minimal Fix**:
  Check `id.trim().is_empty()`, `name.trim().is_empty()`, `system_prompt.trim().is_empty()`.

### GC1-14: Double Clone and Unbounded Map Growth in `PersonaRegistry::from_configs`
- **File & Line**: `src/group_chat/persona.rs:23-36`
- **Issue**:
  `presets` starts with default capacity (0) and repeatedly reallocates, while `cfg.id.clone()` is cloned twice.
- **Proposed Minimal Fix**:
  `HashMap::with_capacity(configs.len())` and single clone.
