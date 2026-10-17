# Task 7 — Phase B1 provider caller migration / Provider 调用点迁移

**Status / 状态：DONE_WITH_CONCERNS — Phase B1 only; NOT Task 7 acceptance.**

English summary: Adapted exactly eleven provider-side exhaustive match sites (six ThinkLevel, five UnifiedMessage) through the existing single-owner dispatch helpers. Provider policy and all known message branches remain in Core. Scoped rustfmt and git diff checks passed; no Cargo/type-check/runtime test was run. Phase B2 and final Task 7 verification remain outstanding.

## 1. Boundary and approach / 边界与方案

- Root: `/Volumes/TBU4/Workspace/Aleph`; branch: `refactor/modularization-p0`.
- Read the Task 7 brief, match-boundary document, source-review §10 under `.superpowers/sdd/2026-10-14-modularization-p0-implementation/`, actual API helper signatures and `task-7-phase-a-report.md`. Phase A's final repair addendum records 261 passed / 0 failed API-lib tests; this is prior-phase evidence, not a B1 execution or Core proof.
- Changed only the five authorized Rust files below and this report. No API/helper/DTO definition, dependency, runtime provider trait, HTTP policy or context/moa source change.
- Used existing Core compatibility reexports of `dispatch_unified_message`, `dispatch_think_level` and `select_think_level_copy`. Callbacks share a mutable accumulator; ThinkLevel value maps use unit context, with model policy captured only by the selected adaptive callback. Anthropic's const mapping retains its const signature via the Copy selector.
- Trade-off / 代价：explicit per-variant callbacks are more verbose, and Anthropic's callback-local return/index reasoning requires care. This avoids consumer catch-all behavior or a second DTO/policy owner; no new dispatch abstraction was added.

## 2. Exact eleven-site mapping / 精确调用点

Line numbers are captured pre-edit match locations → final helper call locations; duplicate paths are intentional.

| Enum / use | Original site | Final site and helper |
|---|---|---|
| ThinkLevel, OpenAI effort | `src/providers/protocols/openai_chat/proto_impl.rs:263` | `src/providers/protocols/openai_chat/proto_impl.rs:261` — `dispatch_think_level` |
| ThinkLevel, Anthropic const budget | `src/providers/protocols/anthropic/proto_impl.rs:474` | `src/providers/protocols/anthropic/proto_impl.rs:476` — `select_think_level_copy` |
| ThinkLevel, Anthropic adaptive effort | `src/providers/protocols/anthropic/proto_impl.rs:594` | `src/providers/protocols/anthropic/proto_impl.rs:597` — `dispatch_think_level` |
| ThinkLevel, Gemini 2.5 budget | `src/providers/protocols/gemini/proto_impl.rs:252` | `src/providers/protocols/gemini/proto_impl.rs:249` — `dispatch_think_level` |
| ThinkLevel, Gemini level | `src/providers/protocols/gemini/proto_impl.rs:268` | `src/providers/protocols/gemini/proto_impl.rs:267` — `dispatch_think_level` |
| ThinkLevel, Responses reasoning | `src/providers/responses/shared.rs:190` | `src/providers/responses/shared.rs:192` — `dispatch_think_level` |
| UnifiedMessage, OpenAI | `src/providers/protocols/openai_chat/proto_impl.rs:93` | `src/providers/protocols/openai_chat/proto_impl.rs:93` — `dispatch_unified_message` |
| UnifiedMessage, Anthropic | `src/providers/protocols/anthropic/proto_impl.rs:87` | `src/providers/protocols/anthropic/proto_impl.rs:87` — `dispatch_unified_message` |
| UnifiedMessage, Gemini | `src/providers/protocols/gemini/proto_impl.rs:60` | `src/providers/protocols/gemini/proto_impl.rs:60` — `dispatch_unified_message` |
| UnifiedMessage, Responses | `src/providers/responses/shared.rs:26` | `src/providers/responses/shared.rs:26` — `dispatch_unified_message` |
| UnifiedMessage, Ollama | `src/providers/ollama.rs:231` | `src/providers/ollama.rs:231` — `dispatch_unified_message` |

## 3. Behavior preservation / 行为保留

### ThinkLevel

All tables use owner order **Off / Minimal / Low / Medium / High / XHigh**:

- OpenAI: `Some("none") / Some("minimal") / Some("low") / Some("medium") / Some("high") / Some("xhigh")`, still owned Strings.
- Anthropic const: **`None / Some(1024) / Some(4096) / Some(10000) / Some(20000) / Some(50000)`**. Source truth is Off → None, not Some(0); the disabled path is not converted into an enabled zero-budget request.
- Anthropic adaptive: `None / low / low / medium / high / conditional`. Only XHigh evaluates the unchanged `Self::supports_xhigh_effort(model)` predicate: true → `xhigh`, false → `max`, never `high`.
- Gemini 2.5: `0 / 500 / 1000 / 2000 / 4000 / 8000`. Other models: `MINIMAL / MINIMAL / LOW / MEDIUM / HIGH / HIGH`. The model predicate, optional fields and both `include_thoughts: Some(*level != ThinkLevel::Off)` expressions remain unchanged; Off still emits false.
- Responses: `None` exits before dispatch via `?`; `Some(Off)` still maps to `none`, not absence. Remaining values are `minimal / low / medium / high / xhigh`. Existing model-dependent `clamp_effort(model, effort)?` and `summary: Some("auto")` remain unchanged.

### UnifiedMessage

- Exactly User / Assistant / ToolResult callbacks; no new unknown/default/drop/panic arm. Existing inner ContentBlock wildcards/refutable matches remain intact.
- Anthropic context owns `(Vec<Message>, usize)`. Single-text User increments the index once and returns from the selected callback (formerly outer-loop continue). Other User and Assistant paths still increment once. ToolResult still scans consecutive results, increments once per consumed result, breaks at the first non-result, and emits one user turn with tool-result blocks before sibling images. No extra outer increment was introduced; the post-run next message is not skipped. Empty placeholders, signed-thinking replay, host-specific reasoning input, ID sanitization and errors remain unchanged.
- Gemini User/Assistant empty-parts guards return only from the selected callback, preserving skip-to-next-input behavior; final all-empty fallback remains outside dispatch. ToolResult grouping into the prior user turn remains unchanged.
- OpenAI, Responses and Ollama preserve role/content/image/tool/reasoning order and existing omission rules. Borrowed tool IDs/names from dispatch use `to_owned()` only where the old `&String::clone()` supplied an owned String. Gemini already receives a content slice, so its existing slice match now uses `match content` rather than `.as_slice()`.
- Responses' pre-existing `ToolDefinition::new` test modifications remain untouched. No file reset or whole-file rewrite was used.

## 4. Verification evidence / 实际验证

Fresh checks after all source edits:

```sh
rustfmt --edition 2021 --config skip_children=true --check \
  src/providers/protocols/openai_chat/proto_impl.rs \
  src/providers/protocols/anthropic/proto_impl.rs \
  src/providers/protocols/gemini/proto_impl.rs \
  src/providers/responses/shared.rs \
  src/providers/ollama.rs

git diff --check -- \
  src/providers/protocols/openai_chat/proto_impl.rs \
  src/providers/protocols/anthropic/proto_impl.rs \
  src/providers/protocols/gemini/proto_impl.rs \
  src/providers/responses/shared.rs \
  src/providers/ollama.rs
```

- Both commands: **exit 0**, no output. Direct rustfmt check did not traverse children or modify files.
- Reviewed the scoped git diff, including the distinction between existing Responses test changes and this migration.
- Compared captured pre-edit/current text: **all five files are byte-identical outside the authorized conversion/mapping function bodies and helper imports**. This includes unrelated existing Responses changes and all tests.
- **15/15 message branch bodies** matched after explicitly normalizing only whitespace, index dereference, callback-return vs loop-continue, slice borrowing and equivalent owned-string conversion. This is a static source comparison, not a behavioral test or compiler proof.
- Recounted the original eleven match sites against exactly five message dispatches, five ThinkLevel dispatches and one const selector. Read back the six value mappings and checked model predicates, Off semantics, clamp and index progression.
- Helper signatures did not obstruct this scope; no API change/ruling was needed.

## 5. Concerns and explicit negatives / 风险与未做

- **B1 Cargo invocations: 0; compiled tests: 0.** rustfmt parses/formats syntax but does not establish callback inference, borrowing, const compatibility or whole-Core compilation. These remain compile uncertainty until the authorized guarded validation after B2.
- Remaining context/moa exhaustive-match adaptations belong to B2; no claim that Core now compiles. External compatibility consumer/example, full Task 7 verification and acceptance are not delivered here. Phase A's API-lib green does not validate these provider callbacks.
- Existing documentation above `src/providers/responses/shared.rs::build_reasoning` says Off and None omit reasoning, while the captured implementation distinguishes them. Preserved the implementation and existing documentation without expanding this migration into a documentation correction.
- No new tests, Cargo/check/clippy/workspace/dependency-graph validation, provider network requests, install, graph rebuild, subagents, commit, staging, unstaging, reset or push. Unrelated working-tree/index changes were not intentionally altered.
- **Delivery / 交付：DONE_WITH_CONCERNS for the bounded eleven-site B1 migration; NOT ACCEPTED for Task 7.**
