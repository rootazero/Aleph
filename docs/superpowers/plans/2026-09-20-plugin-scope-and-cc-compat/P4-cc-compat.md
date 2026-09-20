# Plan — Phase P0 (hook stdin capture spike) + Phase P4 (Claude Code compatibility at the execution end)

> Written 2026-09-20 against `3ddc1f2e7` (+ `35e5f8bca` spec commit; `git diff --stat 3ddc1f2e7 HEAD -- src` is empty, so every `file:line` below is valid at HEAD too).
> Spec sections covered: §3.6 items 1–12, §3.5 G4, §5 (hooks / template / marketplace / ClaudeCache / AgentDef unit tests), §6 `qa/plugins` stages `command` / `exit2` / `cc-cache`, §10 acceptance table.
> Contract names are from `plan-contract.md`; every forced deviation is in **Contract deltas** at the end.
> `src/harness/` has a 0-line diff in every task below (the closest this phase gets is `src/tools/scoped/dispatch.rs` and `src/thinker/layers/agent_role.rs`, both outside the ratchet).

Reading order for the implementer: P0 first (it settles one constant in P4.3), then P4.1 → P4.12, **P4.15**, P4.13, P4.14 in that order (P4.15 was added by the reconciliation round and the `cc-cache` QA stage in P4.13 asserts through it). P4.7 has five sub-tasks (a–e) that must land in order.

> **Anchors after reconciliation (2026-09-20).** This phase runs after P1–P3. Where a task touches code an earlier phase rewrites, it anchors on that phase's plan (plan-P1 P1.7 `slash_effect::plugin_command_skill_infos`, P1.9 `lifecycle.rs::{admit, load_all}`, plan-P2P3 P2.3 `visibility.rs::from_discovery`) and keeps the `3ddc1f2e7` line as history in parentheses. Rulings applied: R4.1–R4.7, G-2 (today's `PluginStatus` names), G-7 (U-b, U-c), G-8.

---

## Phase P0 — hook stdin live capture spike (throwaway, NOT committed)

**Purpose.** Settle by observation, not by documentation, the four facts P4.3 emits: the PostToolUse result key (`tool_result` vs `tool_response`), the exact key set of the PreToolUse / PostToolUse envelopes, the live `permission_mode` spellings, and the shape of `transcript_path`. `scan-cc-plugin-format.md` §8 could not settle the first one (skill doc prose says `tool_result`; the live docs page fetched 2026-09-20 did not surface the PostToolUse schema; dsh's port emits `tool_response`). Do not guess — capture.

**Nothing under `~/.claude/settings.json` is touched.** The capture uses a scratch *project* whose `.claude/settings.local.json` is the only place the hook lives; deleting the scratch directory removes the hook.

- [ ] **Step 1: Create the scratch project with the capture hooks**

```bash
P0=/private/tmp/claude-502/-Volumes-TBU4-Workspace-Aleph/94746d53-da54-4744-869a-649ce8870f82/scratchpad/p0-capture
mkdir -p "$P0/.claude"
printf 'first line of README\nsecond line\n' > "$P0/README.md"
cat > "$P0/.claude/settings.local.json" <<'JSON'
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "Read",
        "hooks": [ { "type": "command",
                     "command": "cat >> \"$CLAUDE_PROJECT_DIR/hook-capture-pre.jsonl\"; echo >> \"$CLAUDE_PROJECT_DIR/hook-capture-pre.jsonl\"" } ] }
    ],
    "PostToolUse": [
      { "matcher": "Read",
        "hooks": [ { "type": "command",
                     "command": "cat >> \"$CLAUDE_PROJECT_DIR/hook-capture-post.jsonl\"; echo >> \"$CLAUDE_PROJECT_DIR/hook-capture-post.jsonl\"" } ] }
    ]
  }
}
JSON
```

(If the Bash hook in this session refuses the heredoc, write the JSON with the Write tool — same content.)

- [ ] **Step 2: Run one Claude Code turn that invokes `Read`, in two permission modes**

```bash
cd "$P0"
claude -p "Use the Read tool to read README.md and reply with its first line only." --permission-mode default
claude -p "Use the Read tool to read README.md and reply with its first line only." --permission-mode plan
claude -p "Use the Read tool to read README.md and reply with its first line only." --permission-mode acceptEdits
```

Expected: three answers of `first line of README`; `hook-capture-pre.jsonl` and `hook-capture-post.jsonl` each hold three JSON objects (one per line). If `-p` mode does not fire hooks on this Claude Code version, run one interactive `claude` session in `$P0` instead and ask the same thing.

- [ ] **Step 3: Read the captured envelopes**

```bash
python3 - "$P0" <<'PY'
import json, sys, pathlib
root = pathlib.Path(sys.argv[1])
for name in ("hook-capture-pre.jsonl", "hook-capture-post.jsonl"):
    print("====", name)
    for line in (root / name).read_text().splitlines():
        if not line.strip():
            continue
        d = json.loads(line)
        print("keys:", sorted(d.keys()))
        print("permission_mode:", d.get("permission_mode"), "| hook_event_name:", d.get("hook_event_name"))
        print("transcript_path:", d.get("transcript_path"))
        for k in ("tool_result", "tool_response"):
            if k in d:
                print("RESULT KEY =", k, "| type:", type(d[k]).__name__, "| preview:", str(d[k])[:160])
PY
```

- [ ] **Step 4: Fill the table and hand it to P4.3**

| Field | Expected (scan §8 / official docs) | Captured value / key set | P4.3 emits |
|---|---|---|---|
| `session_id` | string | (fill) | yes — `HookContext.session_id` (already) |
| `transcript_path` | absolute `.jsonl` path | (fill: is it a JSONL under `~/.claude/projects/<slug>/`?) | yes, only when the file-backend transcript exists (P4.3) |
| `cwd` | project dir | (fill) | yes (already when `working_dir` set; P4.3 adds the fallback) |
| `permission_mode` | one of `default\|plan\|acceptEdits\|auto\|dontAsk\|bypassPermissions` | (fill: the three runs must show `default`, `plan`, `acceptEdits`) | yes, from `ExecTier::cc_permission_mode` (P4.3) |
| `hook_event_name` | `PreToolUse` / `PostToolUse` (PascalCase!) | (fill) | yes — **the spelling the hook was registered under** (user ruling U-b, R4.2): a hook registered as `PreToolUse` gets `PreToolUse`, one registered as `before_tool_call` gets `before_tool_call`. `HookConfig.declared_event` (P4.4) → `HookConfig::event_name()` → the payload builder (P4.3 takes the name as `&str`) |
| `tool_name` | `Read` | (fill) | yes (already) |
| `tool_input` | object | (fill) | yes (already) |
| `tool_use_id` | string | (fill) | **not emitted** — Aleph's `HookContext` carries no tool-use id (DEVIATION #43) |
| `tool_result` **or** `tool_response` (PostToolUse only) | unresolved | **(fill — THE fact this spike exists for)** | yes — `CC_POST_TOOL_RESULT_KEY` in P4.3 must equal the captured key |
| `prompt_id`, `scratchpad_dir`, `effort.level`, `agent_id`, `agent_type` | listed in §8 | (fill which appear) | not emitted (DEVIATION #42) |

- [ ] **Step 5: Remove the capture**

```bash
rm -rf "$P0"
ls "$P0" 2>/dev/null && echo "STILL THERE" || echo "capture removed"
grep -c hook-capture ~/.claude/settings.json || echo "global settings untouched (0 matches)"
```

Nothing is committed from P0. The only durable output is the filled table above (paste it into the lead's thread) and the one constant it decides in P4.3.

---

## Phase P4 — Claude Code compatibility at the execution end

### Task P4.1: One decision derivation for command hooks (exit code 2 = block)

**Files:**
- Modify: `src/extension/hooks/mod.rs:315-368` (`parse_command_output` stays; a new `derive_decision` above it) and `:20-30` (module doc "Command-hook output contract")
- Modify: `src/extension/hooks/json_output.rs:80-100` (new `block_reason_hint`)
- Modify: `src/extension/hooks/executor.rs:697-713` (unchanged behaviour, comment only), `:941-952` (interceptor consumption), `:1128-1147` (observer consumption)
- Test: `src/extension/hooks/mod.rs` (`#[cfg(test)] mod tests`), `src/extension/hooks/executor.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `HookResult`, `PermissionDecision`, `HookKind` (existing), `json_output::apply_json_decision` (existing, `pub(super)`).
- Produces:
  ```rust
  // src/extension/hooks/mod.rs
  pub(crate) fn derive_decision(exit_code: Option<i32>, stdout: &str, stderr: &str, kind: HookKind, result: &mut HookResult);
  pub(crate) const EXIT2_DEFAULT_REASON: &str = "Blocked by hook (exit 2, no reason on stderr).";
  // src/extension/hooks/json_output.rs
  pub(super) fn block_reason_hint(stdout: &str) -> Option<String>;
  ```
  (Contract delta: the contract said "returns the existing hook decision type"; the existing decision type is `HookResult`, which `parse_command_output` already fills by `&mut` accumulator — last-writer-wins across an interceptor chain. Keeping that shape means one accumulator, not a merge function. Recorded below.)

The current code the task replaces — `executor.rs:697-713` (exit status is recorded, never consulted):

```rust
        if !status.success() {
            warn!("Hook command exited with status {:?}", status.code());
        }

        Ok(ActionResult {
            success: status.success(),
            output: if stdout.is_empty() { None } else { Some(stdout) },
            error: if stderr.is_empty() { None } else { Some(stderr) },
            exit_code: status.code(),
        })
```

and `executor.rs:941-952` (only `ar.output` is read; a CC hook written as `echo reason >&2; exit 2` passes silently):

```rust
                        match action {
                            HookAction::Command { .. }
                            | HookAction::Http { .. }
                            | HookAction::Plugin { .. } => {
                                if let Some(ref output) = ar.output {
                                    super::parse_command_output(output, &mut accumulated);
                                    if accumulated.blocked || accumulated.denied {
                                        return Ok((current_context, accumulated));
                                    }
                                }
                            }
```

- [ ] **Step 1: Write the failing tests (pure-function matrix + one real process)**

Append to `src/extension/hooks/mod.rs` inside `mod tests`:

```rust
    /// The exit-code × stdout × kind matrix, on the ONE derivation.
    fn derived(exit: Option<i32>, stdout: &str, stderr: &str, kind: HookKind) -> HookResult {
        let mut r = HookResult::default();
        derive_decision(exit, stdout, stderr, kind, &mut r);
        r
    }

    #[test]
    fn exit_2_on_an_interceptor_blocks_with_stderr_as_the_reason() {
        let r = derived(Some(2), "", "path is outside the repo\n", HookKind::Interceptor);
        assert!(r.blocked);
        assert_eq!(r.block_reason.as_deref(), Some("path is outside the repo"));
        assert_eq!(
            r.permission_decision,
            Some(PermissionDecision::Block { reason: "path is outside the repo".into() })
        );
        assert!(!r.denied, "exit 2 is a retryable block, not a policy deny");
    }

    #[test]
    fn exit_2_with_empty_stderr_still_blocks_with_the_default_reason() {
        let r = derived(Some(2), "", "   \n", HookKind::Interceptor);
        assert!(r.blocked);
        assert_eq!(r.block_reason.as_deref(), Some(EXIT2_DEFAULT_REASON));
    }

    #[test]
    fn exit_2_cannot_be_overridden_by_a_json_allow_on_stdout() {
        // CC: "exit 2 … cannot be overridden by JSON".
        let r = derived(Some(2), r#"{"decision":"approve"}"#, "nope", HookKind::Interceptor);
        assert!(r.blocked, "stdout JSON must not lift an exit-2 block");
        assert_eq!(r.block_reason.as_deref(), Some("nope"));
    }

    #[test]
    fn exit_2_takes_permission_decision_reason_over_stderr() {
        let stdout = r#"{"hookSpecificOutput":{"permissionDecisionReason":"from json"}}"#;
        let r = derived(Some(2), stdout, "from stderr", HookKind::Interceptor);
        assert!(r.blocked);
        assert_eq!(r.block_reason.as_deref(), Some("from json"));
    }

    #[test]
    fn exit_2_on_an_observer_only_logs() {
        // An observer seam never reads the result, so "block" there would be
        // a lie either way; the derivation must not pretend.
        let r = derived(Some(2), "", "reason", HookKind::Observer);
        assert!(!r.blocked);
        assert!(r.permission_decision.is_none());
    }

    #[test]
    fn exit_0_json_deny_is_applied() {
        let r = derived(
            Some(0),
            r#"{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"p"}}"#,
            "",
            HookKind::Interceptor,
        );
        assert!(r.denied);
        assert_eq!(r.deny_reason.as_deref(), Some("p"));
    }

    #[test]
    fn exit_0_line_prefix_block_is_applied() {
        let r = derived(Some(0), "block: not now\n", "", HookKind::Interceptor);
        assert!(r.blocked);
        assert_eq!(r.block_reason.as_deref(), Some("not now"));
    }

    #[test]
    fn other_non_zero_is_non_blocking_and_reads_no_decision_from_stdout() {
        // CC: any other code = non-blocking error; the hook FAILED, so what it
        // printed is diagnostics, not a decision.
        let r = derived(Some(1), "deny: should be ignored\n", "boom", HookKind::Interceptor);
        assert!(!r.blocked && !r.denied);
        assert!(r.permission_decision.is_none());
        assert!(r.messages.is_empty(), "a failed hook's stdout must not become model context");
    }

    #[test]
    fn no_exit_code_parses_stdout_like_before() {
        // HTTP / plugin actions (and a signal-killed command) have no process
        // exit code; they keep the pre-existing "stdout is the decision" path.
        let r = derived(None, r#"{"decision":"block","reason":"r"}"#, "", HookKind::Interceptor);
        assert!(r.blocked);
        assert_eq!(r.block_reason.as_deref(), Some("r"));
    }
```

Append to `src/extension/hooks/executor.rs` inside `mod tests` (next to `oversized_stdout_fails_closed_not_open`):

```rust
    #[cfg(unix)]
    #[tokio::test]
    async fn exit_2_command_hook_blocks_the_interceptor_seam_with_its_stderr() {
        // The most common Claude Code hook idiom. Before this task the exit
        // code was recorded on `ActionResult` and never consulted, so this
        // hook let the tool through.
        use crate::extension::hooks::HookContext;
        let hook = interceptor_command_hook("echo 'outside the repo' >&2; exit 2");
        let executor = HookExecutor::new(vec![hook]);
        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s"))
            .await
            .expect("interceptor pass returns Ok on a hook decision");
        assert!(result.blocked, "exit 2 must block");
        assert_eq!(result.block_reason.as_deref(), Some("outside the repo"));
        assert!(
            !result.action_failed,
            "exit 2 is a hook DECISION, not an infrastructure failure"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exit_1_command_hook_is_non_blocking() {
        use crate::extension::hooks::HookContext;
        let hook = interceptor_command_hook("echo 'deny: nope'; exit 1");
        let executor = HookExecutor::new(vec![hook]);
        let (_ctx, result) = executor
            .execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s"))
            .await
            .expect("Ok");
        assert!(!result.blocked && !result.denied, "exit 1 is non-blocking");
        assert_eq!(result.action_results.len(), 1);
        assert_eq!(result.action_results[0].exit_code, Some(1));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib extension::hooks -- --nocapture`
Expected: compile error `cannot find function derive_decision in this scope` (mod.rs tests) and, once the function stub exists, `exit_2_command_hook_blocks_the_interceptor_seam_with_its_stderr` FAILS with `exit 2 must block`.

- [ ] **Step 3: Write the implementation**

`src/extension/hooks/json_output.rs` — add after `apply_json_decision`:

```rust
/// The reason an exit-2 block should carry when stdout ALSO holds a JSON
/// object: CC reads `hookSpecificOutput.permissionDecisionReason` first and
/// falls back to stderr. Everything else in that object is ignored on exit 2
/// ("cannot be overridden by JSON"), which is why this returns one string and
/// does not touch a `HookResult`.
pub(super) fn block_reason_hint(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let parsed: JsonHookOutput = serde_json::from_str(trimmed).ok()?;
    parsed
        .hook_specific_output
        .and_then(|h| h.permission_decision_reason)
        .filter(|r| !r.trim().is_empty())
}
```

`src/extension/hooks/mod.rs` — add above `parse_command_output` (after the `HookResult` impl):

```rust
/// Reason attached to an exit-2 block whose hook wrote nothing to stderr.
/// A block must never be silent (same rule as `json_output`'s
/// `DEFAULT_BLOCK_MESSAGE`); the wording names the exit code so the model
/// can tell "a script refused" from "a policy refused".
pub(crate) const EXIT2_DEFAULT_REASON: &str = "Blocked by hook (exit 2, no reason on stderr).";

/// Fold one command-hook outcome — process exit code, stdout, stderr — into
/// the accumulated [`HookResult`].
///
/// THE ONE derivation of "what did this hook decide". Both interceptor and
/// observer seams call it (`executor.rs`), and it is the only caller of the
/// two stdout parsers, so the exit-code contract and the stdout contracts
/// cannot disagree. Claude Code's rules (hooks reference, "Exit codes"):
///
/// * exit `0` — stdout IS the decision: a `{…}` object goes through
///   `json_output`, anything else through the line-prefix protocol
///   ([`parse_command_output`]).
/// * exit `2` — **blocking**. Reason = `hookSpecificOutput.permissionDecisionReason`
///   when stdout carries one, else stderr, else [`EXIT2_DEFAULT_REASON`].
///   No other JSON field is applied ("cannot be overridden by JSON").
/// * any other code — non-blocking error: no decision is read from stdout
///   (the hook failed; what it printed is diagnostics), stderr is logged.
///
/// `exit_code == None` means "no process exit code" — HTTP and plugin actions
/// (whose transport already decided success) and a signal-killed command —
/// and keeps the pre-existing behaviour: stdout is parsed as a decision.
///
/// `kind` bounds what a block may do. An [`HookKind::Observer`] runs on
/// seams that never read the result, so exit 2 there is logged at `warn!`
/// and nothing else: a CC hook registered on an observer-only seam must be
/// neither a silent no-op nor a block that the seam would ignore anyway.
pub(crate) fn derive_decision(
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    kind: HookKind,
    result: &mut HookResult,
) {
    match exit_code {
        Some(2) => {
            let reason = json_output::block_reason_hint(stdout)
                .or_else(|| {
                    let s = stderr.trim();
                    (!s.is_empty()).then(|| s.to_string())
                })
                .unwrap_or_else(|| EXIT2_DEFAULT_REASON.to_string());
            match kind {
                HookKind::Interceptor => {
                    result.blocked = true;
                    result.block_reason = Some(reason.clone());
                    result.permission_decision = Some(PermissionDecision::Block { reason });
                }
                HookKind::Observer => tracing::warn!(
                    reason = %reason,
                    "observer hook exited 2 (a blocking exit) on a seam that cannot block; logged only"
                ),
            }
        }
        Some(0) | None => parse_command_output(stdout, result),
        Some(code) => tracing::warn!(
            exit_code = code,
            stderr = %stderr.trim(),
            "hook exited non-zero (non-blocking error); its stdout is not read as a decision"
        ),
    }
}
```

`src/extension/hooks/executor.rs:941-952` — replace the three-arm match head with:

```rust
                        match action {
                            HookAction::Command { .. } => {
                                // Exit code, stdout and stderr go through the
                                // ONE derivation; exit 2 blocks here even when
                                // stdout is empty (the CC `>&2; exit 2` idiom).
                                super::derive_decision(
                                    ar.exit_code,
                                    ar.output.as_deref().unwrap_or(""),
                                    ar.error.as_deref().unwrap_or(""),
                                    HookKind::Interceptor,
                                    &mut accumulated,
                                );
                                if accumulated.blocked || accumulated.denied {
                                    return Ok((current_context, accumulated));
                                }
                            }
                            HookAction::Http { .. } | HookAction::Plugin { .. } => {
                                // `ActionResult::exit_code` is the HTTP status
                                // here (`Some(200)`), not a process code — pass
                                // `None` so the transport's own success verdict
                                // stands and the body is read as the decision,
                                // exactly as before.
                                super::derive_decision(
                                    None,
                                    ar.output.as_deref().unwrap_or(""),
                                    "",
                                    HookKind::Interceptor,
                                    &mut accumulated,
                                );
                                if accumulated.blocked || accumulated.denied {
                                    return Ok((current_context, accumulated));
                                }
                            }
```

(the `Prompt` and `Agent` arms and the trailing `accumulated.action_results.push(ar);` are unchanged.)

`src/extension/hooks/executor.rs:1128-1147` (inside `execute_observers`' per-hook future) — replace

```rust
                for action in &hook.actions {
                    if let Err(e) = self
                        .execute_action(action, context, &hook.plugin_root, &hook.plugin_name, event, timeout_override)
                        .await
                    {
                        warn!("Observer hook action from plugin '{}' failed: {}", hook.plugin_name, e);
                    }
                }
```

with

```rust
                for action in &hook.actions {
                    match self
                        .execute_action(action, context, &hook.plugin_root, &hook.plugin_name, event, timeout_override)
                        .await
                    {
                        Ok(ar) => {
                            // Same derivation as the interceptor seam, with the
                            // observer kind: exit 2 is logged, never applied.
                            // `scratch` is dropped — observers cannot modify.
                            if let HookAction::Command { .. } = action {
                                let mut scratch = super::HookResult::default();
                                super::derive_decision(
                                    ar.exit_code,
                                    ar.output.as_deref().unwrap_or(""),
                                    ar.error.as_deref().unwrap_or(""),
                                    HookKind::Observer,
                                    &mut scratch,
                                );
                            }
                        }
                        Err(e) => warn!(
                            "Observer hook action from plugin '{}' failed: {}",
                            hook.plugin_name, e
                        ),
                    }
                }
```

`src/extension/hooks/mod.rs:20-30` module doc — replace the "Command-hook output contract" paragraph with:

```rust
//! # Command-hook decision contract
//!
//! ONE function, [`derive_decision`], turns a command hook's `(exit code,
//! stdout, stderr)` into a decision. Exit `2` blocks with stderr as the
//! reason (Claude Code's most common hook idiom); exit `0` reads stdout in
//! one of two ways — a JSON decision object (`json_output`, Claude-Code /
//! hermes interop) or the Aleph-native line-prefix protocol
//! ([`parse_command_output`]); any other exit code is a non-blocking error.
//! HTTP and plugin actions have no process exit code and read their body as
//! stdout would be read on exit `0`.
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib extension::hooks -- --nocapture`
Expected: PASS, including every pre-existing `json_output::tests::*`, `interceptor_chain_propagates_rewrite_to_stdin_payload`, `oversized_stdout_fails_closed_not_open`.

**Mutation step (guard discipline):** change `Some(2) =>` to `Some(3) =>` in `derive_decision`; `exit_2_on_an_interceptor_blocks_with_stderr_as_the_reason` and `exit_2_command_hook_blocks_the_interceptor_seam_with_its_stderr` must go red. Revert.

- [ ] **Step 5: Same-commit doc line (R4.6, 判据 §1)**

`docs/reference/FEATURE_LOCATOR.md` §5.10 (`:3010+`): the "**状态**" bullet's hook-round history ends with the paragraph beginning `**未做（backlog）**`. Insert, immediately BEFORE that paragraph, one new paragraph (same indentation as the `①`–`⑥` paragraphs above it):

```markdown
  **exit-2 轮（2026-09-20，CC 兼容补齐 P4.1）**：command hook 的决策此前只读 stdout——`ActionResult.exit_code` 被记录、从未被消费（`rg exit_code src/extension/hooks` 只有写入点），所以 Claude Code 最常见的 hook 写法 `echo reason >&2; exit 2` **静默放行**。现 `hooks::derive_decision(exit_code, stdout, stderr, kind, &mut HookResult)` 是**唯一**的决策派生：exit 2 ⇒ Interceptor 缝 `blocked` + stderr 为理由（stdout 的 JSON 只能提供 `permissionDecisionReason`，不能推翻），Observer 缝只 warn；exit 0 ⇒ stdout 走 JSON / 行前缀两条既有协议；其他非零 ⇒ 非阻塞、stdout 不当决策读；HTTP / plugin 动作传 `None`（它们的 `exit_code` 是 HTTP 状态码）。矩阵测试在 `hooks/mod.rs`，真进程测试 `exit_2_command_hook_blocks_the_interceptor_seam_with_its_stderr`。
```

- [ ] **Step 6: Commit**

```bash
git add src/extension/hooks/mod.rs src/extension/hooks/json_output.rs src/extension/hooks/executor.rs docs/reference/FEATURE_LOCATOR.md
git commit -m "hooks: derive command-hook decisions from exit code (exit 2 = block)

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P4.2: `hookSpecificOutput.updatedInput` through the `update_input:` path

**Files:**
- Modify: `src/extension/hooks/json_output.rs:69-80` (`HookSpecificOutput`), `:107-118` (`apply`)
- Modify: `src/extension/hooks/mod.rs:399-406` (the `update_input:` arm) and the `HookResult` impl (new setter)
- Test: `src/extension/hooks/json_output.rs`, `src/extension/hooks/mod.rs`

**Interfaces:**
- Produces: `impl HookResult { pub(crate) fn set_updated_input(&mut self, input: serde_json::Value) }` — the single writer of `HookResult::updated_input`.

Current code, `hooks/mod.rs:399-406`:

```rust
        } else if let Some(json_str) = trimmed.strip_prefix("update_input:") {
            match serde_json::from_str(json_str.trim()) {
                Ok(val) => result.updated_input = Some(val),
                Err(e) => {
                    tracing::warn!("Hook update_input invalid JSON: {}", e);
                }
            }
```

- [ ] **Step 1: Write the failing tests**

`src/extension/hooks/json_output.rs` `mod tests`:

```rust
    #[test]
    fn hook_specific_updated_input_rewrites_the_tool_input() {
        let json = r#"{"hookSpecificOutput":{"permissionDecision":"allow","updatedInput":{"path":"/rewritten","dry_run":true}}}"#;
        let (consumed, result) = apply(json);
        assert!(consumed);
        assert_eq!(
            result.updated_input,
            Some(serde_json::json!({"path": "/rewritten", "dry_run": true}))
        );
        assert_eq!(result.permission_decision, Some(PermissionDecision::Allow));
    }

    #[test]
    fn hook_specific_block_is_a_retryable_block() {
        // The live docs (scan-cc-plugin-format §8) spell the enum
        // `allow|deny|block`; the shipping skill doc spells it `allow|deny|ask`.
        // Both are accepted; `block` maps to the retryable Block, not Deny.
        let json = r#"{"hookSpecificOutput":{"permissionDecision":"block","permissionDecisionReason":"try later"}}"#;
        let (consumed, result) = apply(json);
        assert!(consumed);
        assert!(result.blocked && !result.denied);
        assert_eq!(result.block_reason.as_deref(), Some("try later"));
        assert_eq!(
            result.permission_decision,
            Some(PermissionDecision::Block { reason: "try later".to_string() })
        );
    }

    #[test]
    fn updated_input_last_writer_wins_like_the_prefix_path() {
        let mut result = HookResult::default();
        apply_json_decision(r#"{"hookSpecificOutput":{"updatedInput":{"a":1}}}"#, &mut result);
        super::super::parse_command_output(r#"update_input: {"a":2}"#, &mut result);
        assert_eq!(result.updated_input, Some(serde_json::json!({"a": 2})));
    }
```

`src/extension/hooks/mod.rs` `mod tests` — the single-writer guard:

```rust
    /// Both spellings of "rewrite the tool input" — the JSON `updatedInput`
    /// and the `update_input:` line — must reach `updated_input` through the
    /// one setter. Source-level, so a third spelling written as a bare field
    /// assignment is red on arrival.
    #[test]
    fn updated_input_has_exactly_one_writer() {
        use crate::utils::source_scan::{code_text, production_prefix};
        let corpus = [
            ("hooks/mod.rs", code_text(&production_prefix(include_str!("mod.rs")))),
            ("hooks/json_output.rs", code_text(&production_prefix(include_str!("json_output.rs")))),
            ("hooks/executor.rs", code_text(&production_prefix(include_str!("executor.rs")))),
        ];
        // (file, line, enclosing fn) for every direct write of the field.
        let mut direct = Vec::new();
        for (name, code) in &corpus {
            let lines: Vec<&str> = code.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if line.contains("updated_input = Some(") || line.contains("updated_input = None") {
                    let enclosing = lines[..i]
                        .iter()
                        .rev()
                        .find_map(|l| l.trim_start().strip_prefix("pub(crate) fn ").or_else(|| l.trim_start().strip_prefix("pub fn ")).or_else(|| l.trim_start().strip_prefix("fn ")))
                        .map(|rest| rest.split('(').next().unwrap_or("?").to_string())
                        .unwrap_or_else(|| "?".to_string());
                    direct.push((name.to_string(), i + 1, enclosing));
                }
            }
        }
        assert_eq!(
            direct.len(),
            1,
            "`updated_input` is written directly at {direct:?}; the only direct write is inside \
             `HookResult::set_updated_input` — route new writers through it"
        );
        assert_eq!(
            direct[0].2, "set_updated_input",
            "the one direct write must be the setter itself, found in fn `{}` at {}:{}",
            direct[0].2, direct[0].0, direct[0].1
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib extension::hooks -- --nocapture`
Expected: `hook_specific_updated_input_rewrites_the_tool_input` FAILS (`updated_input` is `None`: the field is not parsed); `hook_specific_block_is_a_retryable_block` FAILS (`blocked == false`: the `block` arm does not exist). `updated_input_has_exactly_one_writer` FAILS with `the one direct write must be the setter itself, found in fn parse_command_output at hooks/mod.rs:401`.

- [ ] **Step 3: Write the implementation**

`json_output.rs` — extend the nested struct and `apply`:

```rust
struct HookSpecificOutput {
    /// `"allow"` | `"deny"` | `"ask"`.
    permission_decision: Option<String>,
    /// Reason paired with `permission_decision`.
    permission_decision_reason: Option<String>,
    /// Extra context injected as a `<system-reminder>` next turn.
    additional_context: Option<String>,
    /// Replacement tool input (`PreToolUse`). Routed through the SAME setter
    /// as the Aleph-native `update_input:` line — one field, one writer.
    updated_input: Option<serde_json::Value>,
}
```

In `apply_permission_decision` (`json_output.rs:150-175`) add the arm the live docs spell (R4.1 Q5), between `"deny"` and `"ask"`:

```rust
        "block" => {
            // Live-docs spelling of the enum (`allow|deny|block`); a retryable
            // block, the same outcome as the top-level `decision: "block"`.
            let reason = reason.unwrap_or_else(|| DEFAULT_BLOCK_MESSAGE.to_string());
            result.blocked = true;
            result.block_reason = Some(reason.clone());
            result.permission_decision = Some(PermissionDecision::Block { reason });
        }
```

(and the struct doc `/// "allow" | "deny" | "ask".` becomes `/// "allow" | "deny" | "ask" | "block" (both documented spellings of the enum).`)

and in `JsonHookOutput::apply`, inside the `if let Some(hso) = self.hook_specific_output {` block, after `additional_context`:

```rust
            if let Some(input) = hso.updated_input {
                result.set_updated_input(input);
            }
```

`hooks/mod.rs` — in `impl HookResult`:

```rust
    /// Record a hook's rewrite of the tool input. Last writer wins across an
    /// interceptor chain; the interceptor loop then threads the value into
    /// both `HookContext.arguments` and `.tool_input` for the next hook.
    /// The `update_input:` line and the JSON `hookSpecificOutput.updatedInput`
    /// both land here (`updated_input_has_exactly_one_writer`).
    pub(crate) fn set_updated_input(&mut self, input: serde_json::Value) {
        self.updated_input = Some(input);
    }
```

and change the prefix arm to `Ok(val) => result.set_updated_input(val),`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib extension::hooks -- --nocapture`
Expected: PASS.

**Mutation step:** in `json_output.rs` replace `result.set_updated_input(input)` with `result.updated_input = Some(input)`; `updated_input_has_exactly_one_writer` must go red naming `hooks/json_output.rs:<line>`. Revert.

- [ ] **Step 5: Commit**

```bash
git add src/extension/hooks/json_output.rs src/extension/hooks/mod.rs
git commit -m "hooks: parse hookSpecificOutput.updatedInput (one setter) and accept permissionDecision=block

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---
### Task P4.3: stdin payload — `transcript_path`, `permission_mode`, the CC result key, `CLAUDE_PROJECT_DIR`

**Files:**
- Modify: `src/extension/hooks/mod.rs:76-96` (`HookContext` fields + builders), `:566-576` (the struct-literal test `test_substitute_variables` must gain the two fields)
- Modify: `src/extension/hooks/executor.rs:100-150` (`build_event_payload_value`), `:575-606` (env block: add `CLAUDE_PROJECT_DIR`)
- Modify: `src/config/types/policies/exec_tier.rs` (new `ExecTier::cc_permission_mode`, `CC_PERMISSION_MODES` table)
- Modify: `src/gateway/session_store/file_backend/mod.rs:324-326` (`transcript_path` → shared `transcript_file`; new `pub fn transcript_path_for_session`)
- Modify: `src/tools/scoped/dispatch.rs:1441-1459` (`build_hook_context`)
- Modify: `src/gateway/execution_engine/run_loop/project_context.rs:14-22` (`lifecycle_hook_context` gains `permission_mode`), callers `run_loop/mod.rs:617`, `:790` (pass `None`), `run_loop/inner.rs:344`, `:405` (pass `Some(exec_tier.cc_permission_mode())`)
- Modify: `src/bin/aleph-server/commands/hooks.rs:181-193` (`synthetic_payload`)
- Test: `src/extension/hooks/executor.rs` (`mod tests`), `src/config/types/policies/exec_tier.rs` (`mod tests`), `src/gateway/session_store/file_backend/mod.rs` (`mod tests`)

**Interfaces:**
- Consumes: P0's filled table (one constant).
- Produces:
  ```rust
  // src/extension/hooks/executor.rs
  /// Claude Code's PostToolUse result key. P0 settles the spelling; the
  /// official docs say `tool_response`, dsh emits `tool_response`, the
  /// plugin-dev skill prose says `tool_result`. Change THIS constant and the
  /// one assertion that reads it if the capture says otherwise.
  pub(crate) const CC_POST_TOOL_RESULT_KEY: &str = "tool_response";
  // src/extension/hooks/mod.rs
  impl HookContext {
      pub fn with_transcript_path(self, path: Option<PathBuf>) -> Self;
      pub fn with_permission_mode(self, mode: &'static str) -> Self;
  }
  // src/config/types/policies/exec_tier.rs
  impl ExecTier { pub const fn cc_permission_mode(self) -> &'static str; }
  pub const CC_PERMISSION_MODES: [(ExecTier, &str); 4];
  // src/gateway/session_store/file_backend/mod.rs
  pub fn transcript_path_for_session(session_key: &str) -> Option<PathBuf>;
  // src/extension/types/hooks.rs
  impl HookEvent { /// The serde snake_case name (`before_tool_call`) — the Aleph-native spelling.
                   pub fn canonical_name(self) -> String; }
  // src/extension/hooks/executor.rs — the builder is keyed on the NAME, not the enum, so P4.4 can
  // hand it the spelling the hook was registered under (R4.2 / U-b) without touching this file again.
  fn build_event_payload_value(event_name: &str, context: &HookContext) -> serde_json::Value;
  fn build_event_payload(event_name: &str, context: &HookContext) -> String;
  pub fn event_payload_json(event: HookEvent, context: &HookContext) -> String;   // = build_event_payload(&event.canonical_name(), ctx)
  ```

**Mapping table (ExecTier → CC `permission_mode`)** — the code table and this table are the same four rows:

| `ExecTier` | `permission_mode` | Why this pairing |
|---|---|---|
| `Plan` | `plan` | both are read-only until a human approves the plan |
| `Ask` | `default` | CC `default` confirms anything not pre-allowed; Aleph `Ask` confirms every mutating tool |
| `Auto` | `auto` | CC `auto` auto-approves with a guard on dangerous actions; Aleph `Auto` runs everything and stops on the irreversible tail |
| `Full` | `bypassPermissions` | neither asks |

Not producible from Aleph (and therefore never emitted): `acceptEdits` (no "file edits only" tier), `dontAsk` (no "refuse without asking" tier). Open question for the lead: `Auto → auto` vs `Auto → acceptEdits` — the row above picks `auto` on semantics; `acceptEdits` is the more common CC spelling in third-party hook scripts.

Current payload builder (`executor.rs:110-137`) emits `hook_event_name, session_id, tool_name?, tool_input?, tool_output?, tool_error?, cwd?, env?` — no `transcript_path`, no `permission_mode`, no CC result key, and `cwd` only when `working_dir` is set (the tool-dispatch seam never sets it).

- [ ] **Step 1: Write the failing tests**

`src/extension/hooks/executor.rs` `mod tests`:

```rust
    #[test]
    fn post_tool_payload_carries_the_claude_code_envelope_keys() {
        use crate::extension::hooks::HookContext;
        let ctx = HookContext::new("agent:main:ws:1")
            .with_tool_name("file_read")
            .with_tool_input(r#"{"path":"/tmp/x"}"#)
            .with_tool_output("contents")
            .with_tool_error(false)
            .with_working_dir("/work")
            .with_transcript_path(Some(std::path::PathBuf::from("/data/sessions/k/transcript.jsonl")))
            .with_permission_mode("default")
            .with_env("RUN_ID", "r1");
        let json: serde_json::Value =
            serde_json::from_str(&event_payload_json(HookEvent::AfterToolCall, &ctx)).unwrap();
        let mut keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut expected = vec![
            "hook_event_name", "session_id", "tool_name", "tool_input", "tool_output",
            CC_POST_TOOL_RESULT_KEY, "tool_error", "cwd", "transcript_path", "permission_mode", "env",
        ];
        expected.sort_unstable();
        assert_eq!(keys, expected, "payload key set");
        assert_eq!(json["permission_mode"], "default");
        assert_eq!(json["transcript_path"], "/data/sessions/k/transcript.jsonl");
        assert_eq!(json["cwd"], "/work");
        // The CC key mirrors the Aleph-native `tool_output` verbatim — a hook
        // written for either name reads the same text.
        assert_eq!(json[CC_POST_TOOL_RESULT_KEY], json["tool_output"]);
    }

    #[test]
    fn unknown_transcript_and_mode_are_omitted_not_blanked() {
        // A hook must not be handed `""` for a path that does not exist or a
        // mode nobody resolved (`BeforeAgentStart` fires before the tier is
        // known): absent means "unknown", an empty string reads as a value.
        use crate::extension::hooks::HookContext;
        let ctx = HookContext::new("s").with_tool_name("bash");
        let json: serde_json::Value =
            serde_json::from_str(&event_payload_json(HookEvent::BeforeToolCall, &ctx)).unwrap();
        assert!(json.get("transcript_path").is_none());
        assert!(json.get("permission_mode").is_none());
        assert!(json.get(CC_POST_TOOL_RESULT_KEY).is_none(), "no output → no result key");
    }
```

`src/config/types/policies/exec_tier.rs` `mod tests`:

```rust
    #[test]
    fn every_tier_has_one_claude_code_permission_mode_and_the_table_agrees() {
        // The table is what the docs quote; the method is what the payload
        // emits. Derive one from the other so they cannot drift.
        for (tier, mode) in CC_PERMISSION_MODES {
            assert_eq!(tier.cc_permission_mode(), mode);
        }
        let all = [ExecTier::Plan, ExecTier::Ask, ExecTier::Auto, ExecTier::Full];
        assert_eq!(CC_PERMISSION_MODES.len(), all.len(), "one row per tier");
        let mut modes: Vec<&str> = all.iter().map(|t| t.cc_permission_mode()).collect();
        modes.sort_unstable();
        modes.dedup();
        assert_eq!(modes.len(), all.len(), "two tiers must not spell the same mode");
        // The live enum (scan-cc-plugin-format §8): every emitted value is one of these.
        const CC: [&str; 6] = ["default", "plan", "acceptEdits", "auto", "dontAsk", "bypassPermissions"];
        for m in modes { assert!(CC.contains(&m), "{m} is not a Claude Code permission_mode"); }
    }
```

`src/gateway/session_store/file_backend/mod.rs` `mod tests`:

```rust
    #[test]
    fn transcript_path_for_session_answers_only_for_an_existing_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let key = "agent:main:ws:hooks";
        // Not written yet → None (never a path that does not exist).
        assert!(transcript_file(temp.path(), key).is_file() == false);
        std::fs::create_dir_all(transcript_file(temp.path(), key).parent().unwrap()).unwrap();
        std::fs::write(transcript_file(temp.path(), key), "{}\n").unwrap();
        // The store's own spelling and the hook's spelling are the same function.
        let store = FileSessionStore::new(FileSessionStoreConfig {
            base_dir: temp.path().to_path_buf(),
            ..FileSessionStoreConfig::default()
        })
        .unwrap();
        assert_eq!(store.transcript_path(key), transcript_file(temp.path(), key));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib extension::hooks::executor::tests::post_tool -- --nocapture`
Expected: compile errors (`with_transcript_path`, `with_permission_mode`, `CC_POST_TOOL_RESULT_KEY` not found); after stubbing, `payload key set` assertion FAILS (missing `transcript_path` / `permission_mode` / result key).

- [ ] **Step 3: Write the implementation**

`src/config/types/policies/exec_tier.rs` — after `impl ExecTier { … }` (next to `id()`):

```rust
/// Each tier's Claude Code `permission_mode` spelling — the value a command
/// hook reads from its stdin JSON. One row per tier; the method below is
/// derived from this table and the test pins the two together.
pub const CC_PERMISSION_MODES: [(ExecTier, &str); 4] = [
    (ExecTier::Plan, "plan"),
    (ExecTier::Ask, "default"),
    (ExecTier::Auto, "auto"),
    (ExecTier::Full, "bypassPermissions"),
];

impl ExecTier {
    /// The Claude Code `permission_mode` this tier is reported as to hooks
    /// (see [`CC_PERMISSION_MODES`]). `acceptEdits` and `dontAsk` have no
    /// Aleph tier and are never emitted.
    #[must_use]
    pub const fn cc_permission_mode(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Ask => "default",
            Self::Auto => "auto",
            Self::Full => "bypassPermissions",
        }
    }
}
```

`src/gateway/session_store/file_backend/mod.rs` — replace `fn transcript_path` (`:324-326`) with:

```rust
    fn transcript_path(&self, key: &str) -> PathBuf {
        transcript_file(&self.config.base_dir, key)
    }
```

and add at module level (after `sanitize_key_for_dir`):

```rust
/// The transcript file for `session_key` under `base_dir` — the ONE spelling
/// of that path. The store reads/writes through it; the hook payload asks
/// through [`transcript_path_for_session`]. Two spellings would be two
/// answers to "where is this session's transcript".
pub(crate) fn transcript_file(base_dir: &Path, session_key: &str) -> PathBuf {
    base_dir
        .join(sanitize_key_for_dir(session_key))
        .join("transcript.jsonl")
}

/// Where a Claude Code hook can read this session's transcript, if anywhere.
///
/// `Some` only when the file exists: the SQLite backend keeps no file, and
/// the file backend has nothing on disk before the first line is written.
/// Existence is the honest predicate — a hook handed a path must be able to
/// open it. Resolved against the default store location (the only one
/// `aleph-server` constructs, `commands/start/helpers.rs`).
#[must_use]
pub fn transcript_path_for_session(session_key: &str) -> Option<PathBuf> {
    let p = transcript_file(&FileSessionStoreConfig::default().base_dir, session_key);
    p.is_file().then_some(p)
}
```

`src/extension/hooks/mod.rs` — `HookContext` gains two fields (after `tool_error`):

```rust
    /// This session's transcript on disk, when the session store keeps one
    /// (`file_backend::transcript_path_for_session`). `None` on the SQLite
    /// backend or before the first line is written; the payload then OMITS
    /// the key rather than sending `""` — a hook must not be handed a path
    /// that does not exist.
    pub transcript_path: Option<PathBuf>,
    /// This turn's execution tier in Claude Code's `permission_mode`
    /// spelling (`ExecTier::cc_permission_mode`). `None` on seams that fire
    /// before the tier is resolved (`BeforeAgentStart`) and on the global
    /// fire-and-forget observers (gateway / channel / provider events).
    pub permission_mode: Option<&'static str>,
```

builders:

```rust
    /// Set the transcript path (`None` = unknown, key omitted from the payload).
    #[must_use]
    pub fn with_transcript_path(mut self, path: Option<PathBuf>) -> Self {
        self.transcript_path = path;
        self
    }

    /// Set the Claude Code `permission_mode` string for this turn.
    #[must_use]
    pub const fn with_permission_mode(mut self, mode: &'static str) -> Self {
        self.permission_mode = Some(mode);
        self
    }
```

and the struct literal in `test_substitute_variables` (`mod.rs:566-576`) gains `transcript_path: None, permission_mode: None,`.

`src/extension/types/hooks.rs` — in `impl HookEvent`, next to `ALL`:

```rust
    /// The serde snake_case name (`before_tool_call`) — what the payload
    /// carries for a hook that declared no other spelling (runtime/WASM
    /// registrations, `aleph hooks test`). Derived from serde so the enum's
    /// rename attribute stays the single source.
    #[must_use]
    pub fn canonical_name(self) -> String {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::String(s)) => s,
            _ => format!("{self:?}").to_lowercase(),
        }
    }
```

`src/extension/hooks/executor.rs` — `build_event_payload_value` / `build_event_payload` take `event_name: &str` instead of `event: HookEvent` (the `event_str` computation at `:112-115` moves into `canonical_name`); `execute_action` gains an `event_name: &str` parameter right after `event` and passes it to `execute_command`, `execute_http`, `execute_plugin` (all three build the payload); both dispatch loops compute `let event_name = event.canonical_name();` once per hook and pass `&event_name` (P4.4 swaps that one line for `hook.event_name()`). `event_payload_json` keeps its public signature and delegates:

```rust
#[must_use]
pub fn event_payload_json(event: HookEvent, context: &HookContext) -> String {
    build_event_payload(&event.canonical_name(), context)
}
```

`src/extension/hooks/executor.rs` — the constant (next to `MAX_HOOK_OUTPUT_BYTES`):

```rust
/// Claude Code's PostToolUse result key, mirrored from the Aleph-native
/// `tool_output` so a hook written for either name reads the same text.
/// Settled by the P0 live capture (2026-09-20): the official hooks reference
/// and dsh's `hooks-claude-code` port both spell it `tool_response`; the
/// plugin-dev skill's prose (`tool_result`) is stale. If a future capture
/// disagrees, this constant and `post_tool_payload_carries_the_claude_code_envelope_keys`
/// are the only two places to touch.
pub(crate) const CC_POST_TOOL_RESULT_KEY: &str = "tool_response";
```

and in `build_event_payload_value`, after the `tool_output` insert and before `tool_error`:

```rust
    if let Some(o) = &context.tool_output {
        payload.insert("tool_output".into(), Value::String(o.clone()));
        payload.insert(CC_POST_TOOL_RESULT_KEY.into(), Value::String(o.clone()));
    }
```

replace the `cwd` insert with a fallback to the process cwd (the tool-dispatch seam sets no `working_dir`; CC always sends `cwd`):

```rust
    let cwd = context
        .working_dir
        .clone()
        .or_else(|| std::env::current_dir().ok());
    if let Some(c) = cwd {
        payload.insert("cwd".into(), Value::String(c.to_string_lossy().to_string()));
    }
    if let Some(t) = &context.transcript_path {
        payload.insert("transcript_path".into(), Value::String(t.to_string_lossy().to_string()));
    }
    if let Some(m) = context.permission_mode {
        payload.insert("permission_mode".into(), Value::String(m.to_string()));
    }
```

and update the doc comment's schema line to
`{ hook_event_name, session_id, tool_name?, tool_input?, tool_output?, <CC_POST_TOOL_RESULT_KEY>?, tool_error?, cwd, transcript_path?, permission_mode?, env? }`.

In `execute_command` (`executor.rs:575-580`), right after `cmd.env("CLAUDE_PLUGIN_ROOT", plugin_root);`:

```rust
        // Claude Code's project-directory variable (`$CLAUDE_PROJECT_DIR`),
        // the one every CC hook script reaches for first. Same value as the
        // payload's `cwd`.
        cmd.env("CLAUDE_PROJECT_DIR", working_dir);
```

`src/tools/scoped/dispatch.rs:1441-1459` — `build_hook_context` becomes:

```rust
    fn build_hook_context(
        &self,
        name: &str,
        input: &Value,
        tool_output: Option<&str>,
        tool_error: Option<bool>,
    ) -> HookContext {
        let mut ctx = HookContext::new(self.hook_session_id.clone())
            .with_tool_name(name.to_string())
            .with_arguments(input.to_string())
            .with_tool_input(input.to_string())
            // The two Claude Code envelope facts this seam can answer: the
            // transcript file (when the file backend keeps one) and the tier
            // the gate below will enforce, read from the same method the gate
            // reads (`effective_exec_tier`, so a released PlanGate shows).
            .with_transcript_path(
                crate::gateway::session_store::file_backend::transcript_path_for_session(
                    &self.hook_session_id,
                ),
            );
        if let Some(tier) = self.effective_exec_tier() {
            ctx = ctx.with_permission_mode(tier.cc_permission_mode());
        }
        if let Some(out) = tool_output {
            ctx = ctx.with_tool_output(out.to_string());
        }
        if let Some(is_err) = tool_error {
            ctx = ctx.with_tool_error(is_err);
        }
        ctx
    }
```

`src/gateway/execution_engine/run_loop/project_context.rs:14-22`:

```rust
pub(crate) fn lifecycle_hook_context(
    session_id: &str,
    run_id: &str,
    agent: &AgentInstance,
    permission_mode: Option<&'static str>,
) -> HookContext {
    let mut ctx = HookContext::new(session_id)
        .with_env("RUN_ID", run_id)
        .with_env("AGENT_ID", agent.id())
        .with_transcript_path(
            crate::gateway::session_store::file_backend::transcript_path_for_session(session_id),
        );
    if let Some(mode) = permission_mode {
        ctx = ctx.with_permission_mode(mode);
    }
    ctx
}
```

Callers: `run_loop/mod.rs:617` and `:790` (`BeforeAgentStart`, `AgentEnd` — no `exec_tier` in scope at the first; pass `None` at both), `run_loop/inner.rs:344` and `:405` (`SessionStart`, `UserPromptSubmit` — both after `let exec_tier = turn_permissions.tier;` at `:269`) pass `Some(exec_tier.cc_permission_mode())`.

`src/bin/aleph-server/commands/hooks.rs:181-193` `synthetic_payload`:

```rust
    let ctx = HookContext::new("hooks-cli-test")
        .with_tool_name("ExampleTool")
        .with_tool_input(r#"{"example":true}"#)
        .with_permission_mode(alephcore::config::types::policies::ExecTier::default().cc_permission_mode())
        .with_env("ALEPH_HOOKS_TEST", "1".to_string());
```

(no transcript for a synthetic session — the key is omitted, which is the truth.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib extension::hooks -- --nocapture && cargo test -p alephcore --lib policies::exec_tier -- --nocapture && cargo test -p alephcore --lib session_store::file_backend -- --nocapture && cargo test -p alephcore --bins`
Expected: PASS (the `--bins` run compiles `commands/hooks.rs`). `inventory_reports_a_well_formed_hook_as_reachable` still asserts `entry.event == "before_tool_call"` — the inventory row keeps the canonical name; only the stdin payload learns the declared spelling (P4.4).

- [ ] **Step 5: Commit**

```bash
git add src/extension/hooks/mod.rs src/extension/hooks/executor.rs src/extension/types/hooks.rs src/config/types/policies/exec_tier.rs src/gateway/session_store/file_backend/mod.rs src/tools/scoped/dispatch.rs src/gateway/execution_engine/run_loop/project_context.rs src/gateway/execution_engine/run_loop/mod.rs src/gateway/execution_engine/run_loop/inner.rs src/bin/aleph-server/commands/hooks.rs
git commit -m "hooks: emit transcript_path, permission_mode and the CC result key on stdin

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P4.4: Event alignment (32 CC events vs 23 Aleph) + `PermissionDenied` producer + `hook_event_name` echoes the registered spelling + G4 census

**Files:**
- Modify: `src/extension/types/hooks.rs:39-121` (variant docs, `PostCompact` alias, new `PermissionDenied` variant), `:130-155` (`ALL` → 24), `:172-182` (`supports_matcher`), `:434-470` (`HookConfig.declared_event` + `HookConfig::event_name()`)
- Modify: `src/extension/registry/types.rs:73-110` (`HookRegistration.declared_event`), `:491` (test literal)
- Modify: `src/extension/manifest/parsers.rs:77-81` (`HooksFileConfig.hooks` keyed by `String`), `:480-533` (`parse_hooks_content` fills `declared_event`), `:757` (`parse_v2_hooks` literal → `declared_event: None`)
- Modify: `src/extension/hooks/user_settings.rs:186-290` (fills `declared_event` from `event_str`), `:300-317` (`parse_event` becomes `pub(crate)` — the one CC/Aleph-spelling parser, reused by `parsers.rs`)
- Modify: `src/extension/mod.rs:1145-1200` (`sync_hooks_from_registry` passes `declared_event` through), `src/extension/capability.rs:263`, `src/extension/registrar/api.rs:212` (`declared_event: None`)
- Modify: `src/extension/hooks/executor.rs` (the two dispatch loops: `event.canonical_name()` → `hook.event_name()`), plus every `HookConfig { … }` literal (`executor.rs:1163,1392`, `tools/scoped/tests.rs:519`, `memory/session_compactor/prepare_history.rs:295,351`, `verification/extension_stop_gate.rs:328` — `rg -n 'HookConfig \{' src` is the list) gains `declared_event: None`
- Modify: `src/tools/scoped/dispatch.rs:156-162` (`execute_inner` split into wrapper + `execute_gated`)
- Modify: `src/extension/hooks/mod.rs:5-19` (module doc groups)
- Create: `src/extension/hooks/producer_census.rs` (`#[cfg(test)]` only; declared from `hooks/mod.rs`)
- Test: the census file; `src/extension/types/hooks.rs` `mod tests`; `src/tools/scoped/tests.rs`

**Interfaces:**
- Produces: `HookEvent::PermissionDenied` (observer-only; `supports_matcher() == true`), serde alias `PostCompact` on `AfterCompaction`, and:
  ```rust
  // src/extension/registry/types.rs — HookRegistration
  /// The event name exactly as the author wrote it (`PreToolUse` / `before_tool_call`).
  /// `None` for registrations that never had a spelling (WASM runtime API, aleph.plugin.toml [[hooks]]).
  #[serde(default, skip_serializing_if = "Option::is_none")] pub declared_event: Option<String>,
  // src/extension/types/hooks.rs — HookConfig
  #[serde(default, skip_serializing_if = "Option::is_none")] pub declared_event: Option<String>,
  impl HookConfig { /// `declared_event`, else `event.canonical_name()` — THE derivation of the payload's `hook_event_name`.
                    pub fn event_name(&self) -> String; }
  ```

**`hook_event_name` = the spelling the hook was registered under (user ruling U-b, R4.2).** One field on the registration, filled by the two file parsers from the JSON key as written; one derivation (`HookConfig::event_name`); the alias table (P4.5) maps CC → Aleph for dispatch only. An Aleph-native `hooks.json` keyed `before_tool_call` keeps receiving `before_tool_call`; a CC-style one keyed `PreToolUse` receives `PreToolUse`, so a script that does `case "$hook_event_name" in PreToolUse)` works unchanged. The inventory (`HookInventoryEntry.event`) keeps the canonical name — it is a diagnostic view, not the wire the script reads.

**The 32-vs-23 table.** Verdicts: **IMPLEMENTED** (alias already at `types/hooks.rs`), **CONNECT** (this task), **DEVIATION** (documented; the "candidate" rows name the exact site a later round would instrument).

| # | CC event | Aleph event / site | Verdict |
|---|---|---|---|
| 1 | `SessionStart` | `SessionStart` (`run_loop/inner.rs:346`) | IMPLEMENTED |
| 2 | `Setup` | no moment — Aleph has no project-setup verb | DEVIATION: no such moment |
| 3 | `UserPromptSubmit` | `UserPromptSubmit` (`inner.rs:408`) | IMPLEMENTED |
| 4 | `UserPromptExpansion` | the moment appears with P4.7c (command render in `execute.rs`) but CC's hook REWRITES the expansion — a second interceptor seam | DEVIATION this round; candidate site `execution_engine/execute.rs` after `render_command_turn` |
| 5 | `PreToolUse` | `BeforeToolCall` (`dispatch.rs:1246`) | IMPLEMENTED |
| 6 | `PermissionRequest` | `PermissionRequest` (`dispatch.rs:928`) | IMPLEMENTED |
| 7 | `PermissionDenied` | **new** `PermissionDenied`, producer = every `ToolError::PermissionDenied` leaving `execute_inner` (`dispatch.rs:156`) | **CONNECT (this task)** |
| 8 | `PostToolUse` | `AfterToolCall` (`dispatch.rs:1397/1404`) | IMPLEMENTED |
| 9 | `PostToolUseFailure` | `AfterToolCallFailure` (`:1426/1435`) | IMPLEMENTED |
| 10 | `PostToolBatch` | the batch boundary is the harness Act phase (`src/harness/`) | DEVIATION: R10 — no producer outside the ratchet |
| 11 | `Notification` | `Notification` (`dispatch.rs:938`) | IMPLEMENTED |
| 12 | `MessageDisplay` | nearest is `MessageSending` (`channel_registry.rs:635`), but CC's is a display-transform seam (`terminalSequence`) | DEVIATION: not aliased — different contract |
| 13 | `SubagentStart` | `SubagentStart` (`agents/runtime.rs:480`) | IMPLEMENTED |
| 14 | `SubagentStop` | `SubagentStop` (`:569`) | IMPLEMENTED |
| 15 | `TaskCreated` | moment exists: `builtin_tools/task_manage/create.rs:143` (`task_create`) | DEVIATION this round; candidate |
| 16 | `TaskCompleted` | moment exists: `task_manage` complete/submit arms | DEVIATION this round; candidate |
| 17 | `Stop` | `Stop` (`verification/extension_stop_gate.rs:218`) | IMPLEMENTED |
| 18 | `StopFailure` | `AgentEnd` fires on the error path with `AGENT_ERROR` env (`run_loop/mod.rs:792-794`) | DEVIATION: covered by `AgentEnd` + `AGENT_ERROR`, not a separate event |
| 19 | `TeammateIdle` | moment exists: `builtin_tools/team/lifecycle_idle.rs:73` | DEVIATION this round; candidate |
| 20 | `InstructionsLoaded` | CLAUDE.md / AGENTS.md are re-read per turn by `ExtraFilesLayer`; no discrete load moment | DEVIATION: no such moment |
| 21 | `ConfigChange` | moment exists: `gateway/event_bus.rs:498-505` (`publish_gateway_event(ConfigChanged)`, a sync fn) | DEVIATION this round; candidate (needs a spawned task) |
| 22 | `CwdChanged` | moment exists: `SessionStore::set_project_root` (`session_store/mod.rs:378`) | DEVIATION this round; candidate |
| 23 | `DirectoryAdded` | moment exists: `projects.add` RPC | DEVIATION this round; candidate |
| 24 | `FileChanged` | Aleph watches plugin dirs only (`extension/watcher.rs`) | DEVIATION: no such moment |
| 25 | `WorktreeCreate` | Aleph creates subagent worktrees itself (B1 isolation); CC's hook REPLACES the VCS op | DEVIATION: different contract |
| 26 | `WorktreeRemove` | same | DEVIATION |
| 27 | `PreCompact` | `BeforeCompaction` (`session_compactor/prepare_history.rs:258/262`) | IMPLEMENTED |
| 28 | `PostCompact` | `AfterCompaction` (`prepare_history.rs:188`) — alias missing | **CONNECT (this task): serde alias** |
| 29 | `PreModelSwitch` | `select_model` tool → `session_model_handle::set_session_model` (`providers/session_model_handle.rs:123`) | DEVIATION this round; candidate (observer-only — a veto would make a tool an interceptor seam) |
| 30 | `PostModelSwitch` | same site | DEVIATION this round; candidate |
| 31 | `Elicitation` | Aleph's MCP client does not advertise elicitation (`mcp/modern/mod.rs:21,298`) | DEVIATION: no such moment |
| 32 | `ElicitationResult` | same | DEVIATION |

Result after this task: 24 Aleph events, 12 CC names reach one of them (11 aliases + `PermissionDenied`). Unknown event names in a `hooks.json` are already skipped with a warning (`user_settings.rs` module doc) — the "unknown future events no-op" requirement (#39) holds today. **No follow-up round is scheduled for the "candidate" rows (R4.1 Q8); this table is the record.**

- [ ] **Step 1: Write the failing tests**

`src/extension/types/hooks.rs` `mod tests` (create the module if absent — the file has none today):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_compact_is_an_alias_of_after_compaction() {
        let e: HookEvent = serde_json::from_str("\"PostCompact\"").unwrap();
        assert_eq!(e, HookEvent::AfterCompaction);
    }

    #[test]
    fn permission_denied_parses_carries_a_tool_name_and_is_observer_only() {
        let e: HookEvent = serde_json::from_str("\"PermissionDenied\"").unwrap();
        assert_eq!(e, HookEvent::PermissionDenied);
        assert_eq!(serde_json::to_value(e).unwrap(), "permission_denied");
        assert!(e.supports_matcher(), "the refusal names a tool; a matcher can select it");
        assert!(!e.supports_interceptor(), "the refusal has already been returned");
        assert!(HookEvent::ALL.contains(&e));
    }
}
```

`src/extension/hooks/executor.rs` `mod tests` — the declared-spelling test (R4.2):

```rust
    #[cfg(unix)]
    #[tokio::test]
    async fn hook_event_name_echoes_the_spelling_the_hook_was_registered_under() {
        // Two hooks on the same seam, one written the Claude Code way, one the
        // Aleph way; each must read back its OWN spelling from stdin.
        use crate::extension::hooks::HookContext;
        let dir = tempfile::tempdir().unwrap();
        let (cc_out, aleph_out) = (dir.path().join("cc.json"), dir.path().join("aleph.json"));
        let mut cc = interceptor_command_hook(&format!("cat > {}", cc_out.display()));
        cc.declared_event = Some("PreToolUse".into());
        let mut aleph = interceptor_command_hook(&format!("cat > {}", aleph_out.display()));
        aleph.declared_event = Some("before_tool_call".into());
        let mut bare = interceptor_command_hook("true");
        bare.declared_event = None;
        assert_eq!(bare.event_name(), "before_tool_call", "no spelling → the canonical serde name");
        let executor = HookExecutor::new(vec![cc, aleph]);
        executor
            .execute_interceptors(HookEvent::BeforeToolCall, HookContext::new("s").with_tool_name("bash"))
            .await
            .expect("both hooks run");
        let cc_seen: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cc_out).unwrap()).unwrap();
        let aleph_seen: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&aleph_out).unwrap()).unwrap();
        assert_eq!(cc_seen["hook_event_name"], "PreToolUse");
        assert_eq!(aleph_seen["hook_event_name"], "before_tool_call");
    }
```

`src/extension/manifest/parsers.rs` `mod tests` — the parser keeps the key as written:

```rust
    #[test]
    fn hooks_json_keeps_the_event_spelling_the_author_wrote() {
        let caps = parse_hooks_content(
            r#"{"hooks": {"PreToolUse": [{"matcher": "Write", "hooks": [{"type": "command", "command": "a"}]}],
                          "after_tool_call": [{"hooks": [{"type": "command", "command": "b"}]}]}}"#,
            std::path::Path::new("/p"),
            "plug",
        )
        .unwrap();
        let regs: Vec<(HookEvent, Option<String>)> = caps
            .iter()
            .filter_map(|c| match c { CapabilityDeclaration::Hook(h) => Some((h.event, h.declared_event.clone())), _ => None })
            .collect();
        assert!(regs.contains(&(HookEvent::BeforeToolCall, Some("PreToolUse".into()))));
        assert!(regs.contains(&(HookEvent::AfterToolCall, Some("after_tool_call".into()))));
    }
```

`src/tools/scoped/tests.rs` (next to the existing `with_hook_executor` tests at `:552-627`):

```rust
#[cfg(unix)]
#[tokio::test]
async fn a_policy_denial_fires_the_permission_denied_observer() {
    // The refusal used to return before any hook seam: a tier/policy deny was
    // the one gate decision no hook could witness. The observer fires with the
    // tool name and the reason, AFTER the error is decided (observer-only).
    // Same helpers as `before_tool_hook_deny_returns_permission_denied`
    // (`echo_registry`, `make_command_hook`); the deny here comes from an
    // explicit policy entry, not from a hook, so it is the arm no hook could
    // see before.
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("denied.txt");
    let mut hook = make_command_hook(
        HookEvent::PermissionDenied,
        HookKind::Observer,
        &format!("printf '%s' \"$TOOL_NAME:$DENY_REASON\" > {}", marker.display()),
    );
    hook.matcher = Some("echo".into());
    let executor = Arc::new(HookExecutor::new(vec![hook]));
    let svc = ScopedToolService::new(echo_registry(), BTreeSet::new())
        .with_tool_permissions(crate::config::types::policies::ToolPermissionsConfig {
            default: crate::extension::PermissionAction::Allow,
            overrides: std::collections::HashMap::from([(
                "echo".to_string(),
                crate::extension::PermissionAction::Deny,
            )]),
        })
        .with_hook_executor(executor, "test-session");
    let err = svc.execute("echo", json!({})).await.unwrap_err();
    assert!(matches!(err, ToolError::PermissionDenied { .. }), "{err:?}");
    let seen = std::fs::read_to_string(&marker).expect("observer ran");
    assert!(seen.starts_with("echo:"), "tool name reaches the hook: {seen}");
    assert!(seen.len() > "echo:".len(), "reason reaches the hook: {seen}");
}
```

`src/extension/hooks/producer_census.rs` (new, whole file):

```rust
//! G4 — every `HookEvent` has a producer outside `src/extension/`, and every
//! Claude Code alias points at one of them.
//!
//! Derived, not listed: the variant set comes from `HookEvent::ALL`, the
//! alias set from the `#[serde(alias = …)]` attributes in `types/hooks.rs`,
//! and the producers from a walk of `src/` minus `src/extension/` reading
//! production code only (`production_text`, comments stripped). A variant
//! that is declared but never fired — or an alias added for a Claude Code
//! name that nothing in Aleph ever reaches — is red on arrival. The
//! 2026-09-20 scan counted 23/23 by hand; this is that count as a test.

use crate::extension::types::HookEvent;
use crate::utils::source_scan::{production_text, rust_sources_under, strip_comment_lines};

/// `HookEvent::<Variant>` at a word boundary, in production code outside
/// `src/extension/`. Comments are stripped, so a variant named only in a
/// `//` line or a doc comment does not count as fired.
fn producers_of(variant: &str) -> Vec<String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let needle = format!("HookEvent::{variant}");
    let mut hits = Vec::new();
    for (rel, text) in rust_sources_under(&root) {
        if rel.starts_with("src/extension/") {
            continue;
        }
        let prod = strip_comment_lines(&production_text(std::path::Path::new(&rel), &text));
        for (i, line) in prod.lines().enumerate() {
            let mut rest = line;
            while let Some(at) = rest.find(&needle) {
                let after = &rest[at + needle.len()..];
                let boundary = after
                    .chars()
                    .next()
                    .map_or(true, |c| !(c.is_ascii_alphanumeric() || c == '_'));
                if boundary {
                    hits.push(format!("{rel}:{}", i + 1));
                    break;
                }
                rest = after;
            }
        }
    }
    hits
}

/// `(alias, variant)` pairs read off `types/hooks.rs`: an alias attribute
/// applies to the next variant line after it.
fn declared_aliases() -> Vec<(String, String)> {
    let src = include_str!("../types/hooks.rs");
    let enum_body = src
        .split("pub enum HookEvent {")
        .nth(1)
        .and_then(|s| s.split("\n}\n").next())
        .expect("HookEvent enum body");
    let mut pending: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for raw in enum_body.lines() {
        let line = raw.trim();
        if line.starts_with("///") || line.is_empty() {
            continue;
        }
        if line.starts_with("#[serde(") {
            let mut rest = line;
            while let Some(at) = rest.find("alias = \"") {
                let after = &rest[at + "alias = \"".len()..];
                let end = after.find('"').expect("closing quote");
                pending.push(after[..end].to_string());
                rest = &after[end + 1..];
            }
            continue;
        }
        let variant = line.trim_end_matches(',');
        if variant.chars().all(|c| c.is_ascii_alphanumeric()) {
            for alias in pending.drain(..) {
                out.push((alias, variant.to_string()));
            }
        }
    }
    out
}

#[test]
fn every_hook_event_has_a_producer_outside_src_extension() {
    let mut silent = Vec::new();
    for event in HookEvent::ALL {
        let variant = format!("{event:?}");
        if producers_of(&variant).is_empty() {
            silent.push(variant);
        }
    }
    assert!(
        silent.is_empty(),
        "declared hook events with no fire-site outside src/extension/: {silent:?}"
    );
}

#[test]
fn every_claude_code_alias_targets_a_fired_event() {
    let aliases = declared_aliases();
    assert!(aliases.len() >= 24, "alias parse found only {} pairs — the reader rotted", aliases.len());
    let mut dangling = Vec::new();
    for (alias, variant) in &aliases {
        if producers_of(variant).is_empty() {
            dangling.push(format!("{alias} -> {variant}"));
        }
    }
    assert!(dangling.is_empty(), "aliases whose target is never fired: {dangling:?}");
    // The two this round adds are visible to the reader (a reader that
    // cannot see them cannot guard them).
    assert!(aliases.iter().any(|(a, v)| a == "PostCompact" && v == "AfterCompaction"));
    assert!(aliases.iter().any(|(a, v)| a == "PermissionDenied" && v == "PermissionDenied"));
}

#[test]
fn all_lists_every_variant_exactly_once() {
    // `ALL` is the roster the census walks; a variant left out of it would
    // also be left out of the census. Round-trip every declared variant name
    // read off the enum source through serde and back into `ALL`.
    let src = include_str!("../types/hooks.rs");
    let body = src.split("pub enum HookEvent {").nth(1).and_then(|s| s.split("\n}\n").next()).unwrap();
    let declared: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("///") && !l.starts_with("#["))
        .map(|l| l.trim_end_matches(','))
        .collect();
    assert_eq!(declared.len(), HookEvent::ALL.len(), "declared {declared:?} vs ALL");
    for name in declared {
        let e: HookEvent = serde_json::from_str(&format!("\"{name}\"")).unwrap_or_else(|_| panic!("{name} parses via its PascalCase alias"));
        assert!(HookEvent::ALL.contains(&e), "{name} missing from ALL");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib extension::types::hooks -- --nocapture`
Expected: `post_compact_is_an_alias_of_after_compaction` FAILS (`unknown variant PostCompact`); `permission_denied_*` fails to compile (no variant). The census fails to compile until the module is declared; once declared, `every_claude_code_alias_targets_a_fired_event` FAILS on the two `assert!(aliases.iter().any(…))`.

- [ ] **Step 3: Write the implementation**

`src/extension/types/hooks.rs`:

```rust
    /// After session compaction
    #[serde(alias = "AfterCompaction", alias = "PostCompact")]
    AfterCompaction,
```

and, after `PermissionRequest`:

```rust
    /// A tool call was refused by a permission gate — the tier rule, an
    /// explicit `[policies.tool_permissions]` deny, the operator gate, or a
    /// BeforeToolCall hook's `deny:`. Observer-only: the refusal has already
    /// been returned to the model; hooks witness it (audit, metrics, a
    /// notification). Claude Code `PermissionDenied` parity. Carries
    /// `tool_name` (so a `matcher` applies) and `DENY_REASON` in `env`.
    #[serde(alias = "PermissionDenied")]
    PermissionDenied,
```

`ALL` becomes `[Self; 24]` with `Self::PermissionDenied,` inserted after `Self::PermissionRequest,`. `supports_matcher` gains `| PermissionDenied`. (`supports_interceptor` unchanged — observer-only.)

`HookConfig` (`types/hooks.rs:434-470`) gains, after `timeout_secs`:

```rust
    /// The event name exactly as the hook's author wrote it (`PreToolUse`
    /// or `before_tool_call`) — what the stdin payload's `hook_event_name`
    /// echoes back (user ruling 2026-09-20 U-b). `None` for registrations
    /// that never had a spelling (runtime/WASM API, `aleph.plugin.toml`
    /// `[[hooks]]`), which read the canonical serde name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_event: Option<String>,
```

and the one derivation:

```rust
impl HookConfig {
    /// The `hook_event_name` this hook's payload carries: the declared
    /// spelling when there is one, else the canonical serde name.
    #[must_use]
    pub fn event_name(&self) -> String {
        self.declared_event
            .clone()
            .unwrap_or_else(|| self.event.canonical_name())
    }
}
```

`HookRegistration` (`registry/types.rs:73-110`) gains the same `declared_event: Option<String>` field (doc in Interfaces). `parsers.rs:77-81`:

```rust
#[derive(Debug, Deserialize)]
struct HooksFileConfig {
    /// Keyed by the event name AS WRITTEN — a `HashMap<HookEvent, _>` key
    /// would parse the alias and forget the spelling the payload must echo.
    #[serde(default)]
    hooks: HashMap<String, Vec<HookMatcher>>,
}
```

and in `parse_hooks_content` (`:495-533`) the loop head becomes:

```rust
    for (event_str, matchers) in config.hooks {
        // ONE parser for both spellings (`user_settings::parse_event`): an
        // unknown name is skipped with a warn, same as the user-hooks loader.
        let Some(event) = crate::extension::hooks::parse_event(&event_str) else {
            warn!(plugin = plugin_id, event = %event_str, "Unknown hook event in hooks.json; skipping");
            continue;
        };
```

with `declared_event: Some(event_str.clone()),` added to the `HookRegistration { … }` literal at `:513-530` (and `declared_event: None` at `parsers.rs:757`, `capability.rs:263`, `registrar/api.rs:212`, `registry/types.rs:491`). `user_settings.rs:300 fn parse_event` becomes `pub(crate) fn parse_event` (and `pub(crate) use user_settings::parse_event;` in `hooks/mod.rs`); its `HookConfig { … }` literal at `:283` gains `declared_event: Some(event_str.clone()),`. `mod.rs:1148-1200` destructures `declared_event` from the registration and writes it into the `HookConfig`. In `executor.rs`, the two dispatch loops replace P4.3's `let event_name = event.canonical_name();` with `let event_name = hook.event_name();`.

`src/tools/scoped/dispatch.rs:156-162` — rename the existing function body to `execute_gated` and add the wrapper:

```rust
    /// The gated dispatch, plus the one observation no gate could make
    /// before: a `PermissionDenied` leaving here fires the
    /// `HookEvent::PermissionDenied` observers. Placed on the way OUT rather
    /// than at each of the four deny arms (tier rule, operator gate ×2, hook
    /// deny) so a fifth arm is covered without knowing this seam exists.
    /// The input is cloned only when a hook executor is attached.
    pub(super) async fn execute_inner(
        &self,
        name: &str,
        input: Value,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        let for_hook = self
            .hook_executor
            .as_ref()
            .filter(|e| e.hook_count() > 0)
            .map(|e| (e.clone(), input.clone()));
        let result = self.execute_gated(name, input, cancel).await;
        if let (Err(ToolError::PermissionDenied { name: denied, reason }), Some((executor, input))) =
            (&result, for_hook)
        {
            let ctx = self
                .build_hook_context(denied, &input, None, None)
                .with_env("DENY_REASON", reason.clone());
            executor
                .execute_observers(HookEvent::PermissionDenied, &ctx)
                .await;
        }
        result
    }

    async fn execute_gated(
        &self,
        name: &str,
        input: Value,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        // (the former `execute_inner` body, unchanged)
```

`src/extension/hooks/mod.rs` — declare the census and update the doc group:

```rust
//! - Approval: `PermissionRequest` / `PermissionDenied` / `Notification`
…
#[cfg(test)]
mod producer_census;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib extension:: -- --nocapture && cargo test -p alephcore --lib tools::scoped -- --nocapture && cargo test -p alephcore --lib builtin_tools::hooks_manage -- --nocapture && cargo test -p alephcore --lib memory::session_compactor -- --nocapture && cargo test -p alephcore --lib verification::extension_stop_gate -- --nocapture`
Expected: PASS. `hooks_manage::tests::every_event_appears_exactly_once_in_the_catalogue` now counts 24 rows (it derives from `ALL`); `hook_event_name_echoes_the_spelling_the_hook_was_registered_under` and `hooks_json_keeps_the_event_spelling_the_author_wrote` pass.

**Mutation steps (G4):**
1. Comment out the `executor.execute_observers(HookEvent::PermissionDenied, &ctx).await;` line in `dispatch.rs` → `every_hook_event_has_a_producer_outside_src_extension` red with `["PermissionDenied"]`, and `a_policy_denial_fires_the_permission_denied_observer` red. Revert.
2. Add `#[serde(alias = "WorktreeCreate")]` on a new variant `WorktreeCreate` with no fire site (and add it to `ALL`) → `every_claude_code_alias_targets_a_fired_event` red with `["WorktreeCreate -> WorktreeCreate"]`. Revert.
3. Remove `Self::PermissionDenied` from `ALL` → `all_lists_every_variant_exactly_once` red (`declared 24 vs ALL 23`). Revert.
4. In `HookConfig::event_name` return `self.event.canonical_name()` unconditionally → `hook_event_name_echoes_the_spelling_the_hook_was_registered_under` red on `"PreToolUse"`. Revert.

- [ ] **Step 5: Commit**

```bash
git add src/extension/types/hooks.rs src/extension/registry/types.rs src/extension/manifest/parsers.rs src/extension/hooks/user_settings.rs src/extension/hooks/executor.rs src/extension/mod.rs src/extension/capability.rs src/extension/registrar/api.rs src/tools/scoped/dispatch.rs src/tools/scoped/tests.rs src/memory/session_compactor/prepare_history.rs src/verification/extension_stop_gate.rs src/extension/hooks/mod.rs src/extension/hooks/producer_census.rs
git commit -m "hooks: PermissionDenied event, PostCompact alias, hook_event_name echoes the registered spelling, producer census (G4)

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P4.5: Claude Code tool names in matchers (`Bash`, `Read`, `mcp__…`) — one alias table

**Files:**
- Create: `src/extension/hooks/cc_tool_aliases.rs`
- Modify: `src/extension/hooks/mod.rs` (`mod cc_tool_aliases;` + `pub(crate) use`), `src/extension/hooks/executor.rs:311-340` (`matches_pattern`)
- Test: `src/extension/hooks/cc_tool_aliases.rs`, `src/extension/hooks/executor.rs`

**Interfaces:**
- Produces:
  ```rust
  // src/extension/hooks/cc_tool_aliases.rs
  pub(crate) const CC_TOOL_ALIASES: &[(&str, &str)];              // (claude_code_name, aleph_name)
  pub(crate) fn aleph_name(cc: &str) -> Option<&'static str>;     // CC → Aleph (exact, case-sensitive)
  pub(crate) fn cc_spellings(aleph: &str) -> Vec<String>;         // Aleph → every CC spelling (incl. `mcp__<srv>__<tool>`)
  pub(crate) fn normalize_cc_tool_entry(entry: &str, fold_scoped_bash: bool) -> Option<String>;
  ```
  Contract delta: the contract says the table is "used only in the matcher module". P4.7b (command `allowed-tools`) and P4.8 (agent `tools`) must apply the SAME table or they reject every CC plugin (`register_skills` refuses unknown names, `registration.rs:245-263`). One table, three readers, all in this module — recorded below.

**The alias table — every Aleph name verified against `const NAME: &'static str` in `src/builtin_tools/` / `src/tools/` at 3ddc1f2e7 (`rg "const NAME: &'static str = \"" src/builtin_tools src/tools`) and `SUBAGENT_TOOL_NAME` (`agents/subagent_tool/mod.rs:47`):**

| CC name | Aleph name | Verified |
|---|---|---|
| `Bash` | `bash` | `builtin_tools/bash_exec.rs:365` |
| `Read` | `file_read` | NAME list |
| `Write` | `file_write` | NAME list |
| `Edit` | `file_edit` | NAME list |
| `MultiEdit` | `file_edit` | (CC deprecated MultiEdit into Edit) |
| `Glob` | `find` | NAME list |
| `Grep` | `grep` | NAME list |
| `WebFetch` | `web_fetch` | NAME list |
| `WebSearch` | `search` | NAME list (`builtin_tools/search.rs`) |
| `Task` / `Agent` | `subagent` | `SUBAGENT_TOOL_NAME` |
| `AskUserQuestion` | `ask_user` | NAME list |
| `ToolSearch` | `tool_search` | NAME list |

Documented gaps (no Aleph counterpart; a matcher naming them matches nothing, which is the correct fail-closed answer): `NotebookEdit`, `LS` (Aleph folds it into `file_ops list`), `Skill` (CC invokes a skill; Aleph `skill_read` reads one — different verb), `SlashCommand`, `TodoWrite` (Aleph `scratchpad` is a different shape), `KillShell`, `BashOutput`, `EnterWorktree`/`ExitWorktree`.

**MCP names — one derivation.** Aleph registers an MCP tool as `{server_id}__{tool}` (`tools/handlers/mcp.rs:103-110`, `sanitize_tool_name`), and a plugin's `.mcp.json` server keeps its bare key as `server_id` (`loader.rs:109-122 all_mcp_configs_map`, no plugin prefix). CC spells the same tool `mcp__<server>__<tool>` (user-level) or `mcp__plugin_<plugin>_<server>__<tool>` (plugin-level). The matcher therefore tests the regex against the Aleph name AND against `mcp__{server}__{tool}` — derived from the Aleph name by splitting on the first `__`. The `mcp__plugin_<p>_<s>__` spelling is NOT reconstructed (the executor has no registry to learn which plugin owns a server); a CC matcher written that way must use the `mcp__.*` wildcard form. DEVIATION #58 in §10.

- [ ] **Step 1: Write the failing tests**

`src/extension/hooks/cc_tool_aliases.rs` `mod tests`:

```rust
    #[test]
    fn every_aleph_target_is_a_real_tool_name() {
        // The table is hand-written; the registry is not. Every right-hand
        // side must be a name the executor can dispatch, or the alias maps a
        // CC hook onto nothing while looking like it works.
        // `BuiltinToolDefinition.name` is `&'static str`; `subagent` and
        // `tool_search` are the two dispatchable names that live outside that
        // catalog (`SUBAGENT_TOOL_NAME`, `ToolSearchTool::NAME`).
        let known: std::collections::HashSet<&str> = crate::executor::BUILTIN_TOOL_DEFINITIONS
            .iter()
            .map(|d| d.name)
            .chain([
                crate::agents::subagent_tool::SUBAGENT_TOOL_NAME,
                crate::tools::tool_search::ToolSearchTool::NAME,
            ])
            .collect();
        for (cc, aleph) in CC_TOOL_ALIASES {
            assert!(known.contains(aleph), "{cc} -> {aleph}: no such Aleph tool");
        }
    }

    #[test]
    fn cc_to_aleph_is_exact_and_case_sensitive() {
        assert_eq!(aleph_name("Bash"), Some("bash"));
        assert_eq!(aleph_name("Read"), Some("file_read"));
        assert_eq!(aleph_name("bash"), None, "an Aleph name is not a CC name");
        assert_eq!(aleph_name("NotebookEdit"), None, "documented gap");
    }

    #[test]
    fn aleph_to_cc_yields_every_spelling_including_mcp() {
        let mut bash = cc_spellings("bash");
        bash.sort();
        assert_eq!(bash, vec!["Bash"]);
        let mut edit = cc_spellings("file_edit");
        edit.sort();
        assert_eq!(edit, vec!["Edit", "MultiEdit"]);
        assert_eq!(cc_spellings("github__delete_repo"), vec!["mcp__github__delete_repo"]);
        assert!(cc_spellings("scratchpad").is_empty(), "no alias, no spelling");
    }

    #[test]
    fn allowed_tools_entries_fold_by_mode() {
        // `Bash(gh pr view:*)` — CC's per-argument scoping. Restrict mode folds
        // it to bare `bash` (the tier gate still applies to the call);
        // pre-grant mode DROPS it (pre-granting all of bash for a scoped grant
        // would widen an approval skip).
        assert_eq!(normalize_cc_tool_entry("Bash(gh pr view:*)", true).as_deref(), Some("bash"));
        assert_eq!(normalize_cc_tool_entry("Bash(gh pr view:*)", false), None);
        assert_eq!(normalize_cc_tool_entry("Read", true).as_deref(), Some("file_read"));
        assert_eq!(normalize_cc_tool_entry("grep", true).as_deref(), Some("grep"), "Aleph names pass through");
        assert_eq!(normalize_cc_tool_entry("mcp__srv__tool", true).as_deref(), Some("srv__tool"));
        assert_eq!(normalize_cc_tool_entry("mcp__plugin_x_srv__tool", true).as_deref(), Some("srv__tool"));
        assert_eq!(normalize_cc_tool_entry("*", true).as_deref(), Some("*"));
    }
```

`src/extension/hooks/executor.rs` `mod tests`:

```rust
    #[test]
    fn a_claude_code_matcher_selects_the_aleph_tool() {
        use crate::extension::hooks::HookContext;
        let mut hook = dummy_hook("user:global");
        hook.event = HookEvent::BeforeToolCall;
        hook.matcher = Some("Edit|Write".into());
        let exec = HookExecutor::new(vec![hook.clone()]);
        assert!(exec.matches_pattern(&hook, &HookContext::new("s").with_tool_name("file_write")));
        assert!(exec.matches_pattern(&hook, &HookContext::new("s").with_tool_name("file_edit")));
        assert!(!exec.matches_pattern(&hook, &HookContext::new("s").with_tool_name("file_read")));
        // An MCP tool under its CC spelling.
        let mut mcp = dummy_hook("user:global");
        mcp.event = HookEvent::BeforeToolCall;
        mcp.matcher = Some("mcp__.*__delete.*".into());
        let exec = HookExecutor::new(vec![mcp.clone()]);
        assert!(exec.matches_pattern(&mcp, &HookContext::new("s").with_tool_name("github__delete_repo")));
        assert!(!exec.matches_pattern(&mcp, &HookContext::new("s").with_tool_name("github__list_repos")));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib extension::hooks -- --nocapture`
Expected: compile error (no module); after declaring, `a_claude_code_matcher_selects_the_aleph_tool` FAILS on the first `assert!` (`Edit|Write` does not match `file_write`).

- [ ] **Step 3: Write the implementation**

`src/extension/hooks/cc_tool_aliases.rs`:

```rust
//! Claude Code tool names ↔ Aleph tool names — the one table.
//!
//! Three readers, one derivation: the hook matcher (`executor.rs`,
//! `matches_pattern`), a CC command's `allowed-tools:` (P4.7b) and a CC
//! agent's `tools:` (P4.8). Each reader used to be a place where `Read` would
//! silently name nothing. Every right-hand side is checked against the real
//! tool registry by `every_aleph_target_is_a_real_tool_name`.

/// `(claude_code_name, aleph_name)`. Left side is exact and case-sensitive
/// (CC matchers are). Names with no Aleph counterpart are deliberately
/// absent — see the module doc in `PLUGIN_SYSTEM.md` for the list.
pub(crate) const CC_TOOL_ALIASES: &[(&str, &str)] = &[
    ("Bash", "bash"),
    ("Read", "file_read"),
    ("Write", "file_write"),
    ("Edit", "file_edit"),
    ("MultiEdit", "file_edit"),
    ("Glob", "find"),
    ("Grep", "grep"),
    ("WebFetch", "web_fetch"),
    ("WebSearch", "search"),
    ("Task", "subagent"),
    ("Agent", "subagent"),
    ("AskUserQuestion", "ask_user"),
    ("ToolSearch", "tool_search"),
];

/// Claude Code → Aleph, or `None` for a name with no counterpart.
pub(crate) fn aleph_name(cc: &str) -> Option<&'static str> {
    CC_TOOL_ALIASES.iter().find(|(c, _)| *c == cc).map(|(_, a)| *a)
}

/// Every Claude Code spelling of an Aleph tool name — the table's reverse
/// plus the MCP form: Aleph's `{server}__{tool}` is CC's `mcp__{server}__{tool}`.
/// (The plugin-level `mcp__plugin_<p>_<s>__` form needs the owning plugin,
/// which this module cannot know; matchers for it use `mcp__.*`.)
pub(crate) fn cc_spellings(aleph: &str) -> Vec<String> {
    let mut out: Vec<String> = CC_TOOL_ALIASES
        .iter()
        .filter(|(_, a)| *a == aleph)
        .map(|(c, _)| (*c).to_string())
        .collect();
    if let Some((server, tool)) = aleph.split_once("__") {
        if !server.is_empty() && !tool.is_empty() && !aleph.starts_with("mcp__") {
            out.push(format!("mcp__{server}__{tool}"));
        }
    }
    out
}

/// One `allowed-tools:` / `tools:` entry, as Claude Code writes it, into the
/// Aleph name the registries know.
///
/// * `Bash(gh pr view:*)` — CC's per-argument scoping. `fold_scoped_bash`
///   decides its fate: `true` (restrict semantics — a command's
///   `allowed-tools`) folds it to bare `bash`, the tier gate still governs
///   the call; `false` (pre-grant semantics — a skill's `allowed-tools`)
///   returns `None`, because pre-granting all of `bash` for a scoped grant
///   widens an approval skip.
/// * `mcp__<server>__<tool>` and `mcp__plugin_<plugin>_<server>__<tool>` →
///   `<server>__<tool>`.
/// * an Aleph name, or `*`, passes through unchanged.
pub(crate) fn normalize_cc_tool_entry(entry: &str, fold_scoped_bash: bool) -> Option<String> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    if let Some((head, _scope)) = entry.split_once('(') {
        return if fold_scoped_bash { aleph_name(head.trim()).map(str::to_string) } else { None };
    }
    if let Some(rest) = entry.strip_prefix("mcp__") {
        let (server, tool) = rest.rsplit_once("__")?;
        let server = server
            .strip_prefix("plugin_")
            .and_then(|s| s.split_once('_').map(|(_, srv)| srv))
            .unwrap_or(server);
        return Some(format!("{server}__{tool}"));
    }
    Some(aleph_name(entry).map_or_else(|| entry.to_string(), str::to_string))
}
```

`src/extension/hooks/mod.rs`: `mod cc_tool_aliases;` and `pub(crate) use cc_tool_aliases::{aleph_name, cc_spellings, normalize_cc_tool_entry, CC_TOOL_ALIASES};`.

`src/extension/hooks/executor.rs:311-340` `matches_pattern` — replace the three `re.is_match(tool_name)` sites with one helper:

```rust
        // Test the regex against the Aleph name AND every Claude Code spelling
        // of it (`Edit` for `file_edit`, `mcp__srv__tool` for `srv__tool`), so
        // a matcher copied from a CC `settings.json` selects the tool it names.
        let candidates: Vec<String> = std::iter::once(tool_name.clone())
            .chain(super::cc_spellings(tool_name))
            .collect();
        let hit = |re: &regex::Regex| candidates.iter().any(|c| re.is_match(c));
        match self.regex_cache.get(matcher.as_str()) {
            Some(Some(re)) => hit(re),
            Some(None) => false,
            None => match crate::security::safe_regex::bounded_builder(matcher).build() {
                Ok(re) => hit(&re),
                Err(e) => { warn!("Invalid hook matcher regex '{}': {}", matcher, e); false }
            },
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib extension::hooks -- --nocapture`
Expected: PASS, including `inventory_reports_a_well_formed_hook_as_reachable` (its `Write|Edit` matcher is unchanged text).

**Mutation step:** remove the `("Write", "file_write")` row → `a_claude_code_matcher_selects_the_aleph_tool` red on `file_write`. Change `("Read", "file_read")` to `("Read", "file_reed")` → `every_aleph_target_is_a_real_tool_name` red. Revert both.

- [ ] **Step 5: Commit**

```bash
git add src/extension/hooks/cc_tool_aliases.rs src/extension/hooks/mod.rs src/extension/hooks/executor.rs
git commit -m "hooks: match Claude Code tool names (Bash/Read/mcp__…) via one alias table

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P4.6: Hook timeout DEVIATION — no code, one sentence for the P8 docs task

No code change. `MAX_HOOK_TIMEOUT_SECS = 300` (`hooks/mod.rs:71`) and `DEFAULT_COMMAND_TIMEOUT_SECS = 300` (`:59`) stay. Claude Code's live default is 600 s (`scan-cc-plugin-format.md` §8 "Timeouts").

Exact sentence for `docs/reference/PLUGIN_SYSTEM.md`, appended as a new row + note directly under the "支持的组件类型" table (`:369-382`, before the `---` at `:383`):

```markdown
| `hooks/hooks.json` `timeout` | ⚠️ 偏离 | Claude Code 默认 600 s；Aleph 默认 **300 s** 且上限 **300 s**（`MAX_HOOK_TIMEOUT_SECS`，`src/extension/hooks/mod.rs`）——hook 在工具派发内运行，本就受 180 s tool budget 约束，更长的值会被钳到 300 并记一条 warn。写 `timeout: 600` 不报错，只是拿不到 600。 |
```

and, for the acceptance table, the English form: *"DEVIATION #50: hook `timeout` defaults to 300 s and is clamped at 300 s (Claude Code: 600 s). A hook runs inside tool dispatch, which is itself bounded by the 180 s tool budget; a declared `timeout` above the ceiling is clamped with a `warn!` (`HookExecutor::effective_timeout`), never rejected."*

The lead folds this into P8; nothing to commit here.

---
### Task P4.7: `commands/*.md` body reaches the model (five sub-tasks, in order)

Today's wire, verified: `commands/<name>.md` → `parse_single_command` (`manifest/parsers.rs:385-396`) → `SkillRegistration { skill_type: Command, content: <body>, source_path: <md path>, plugin_id }` (`registry/types.rs:183-225`; the body is already in memory as `content`, the file path as `source_path`) → registered by `PluginRegistry::register_skill` under `plugin:name` (`plugin_registry/mod.rs:309-320`) → projected to a slash entry (at 3ddc1f2e7 once at boot, `tool_catalog_init.rs:214-247`; **after P1** per mount by `slash_effect::plugin_command_skill_infos` — plan-P1 P1.7 — with the identical `SkillInfo { id: cmd.qualified_name(), allowed_tools: None, … }` projection; **no frontmatter beyond `name`/`description` is parsed**, `SkillFm` at `parsers.rs:51-60`) → `/name args` resolves to `CommandContext::Skill { skill_id: "plugin:name", allowed_tools }` → `execute.rs:776-782` stamps `allowed_tools` → `execute_slash_command_fast_path` `"skill"` arm falls through (`slash_command.rs:249-275`) → the agent loop runs on the raw `/name args` text. `SkillTemplate` (`template.rs`) has zero consumers (`rg SkillTemplate src` → its file + the `mod.rs:64` re-export).

Where the body is injected: the run loop already delivers per-turn content to the model without persisting it — `transient_blocks` (`run_loop/inner.rs:396-397`, joined at `:510-511`, merged into `HarnessDeps::recall_context` by `harness_bridge/runner_impl.rs:562-578` "appended as a transient user message each Think — delivered to the model but NEVER persisted, so the stored user turn (and the session title) stays equal to the raw input"). The rendered command body rides that channel, wrapped as `<command …>`, pushed FIRST so it precedes the hook reminders. The raw `/name args` stays the persisted user turn. (Open question for the lead: CC persists the *expanded* prompt as the user message; this plan keeps Aleph's raw-input invariant and documents the difference — DEVIATION #16 wording in §10.)

Slash registration / unregistration on plugin mount/unmount is P1's `"slash_command"` effect (`unregister_skills_for_plugin`); nothing here re-does it. P4.7b only changes WHAT the boot-time `SkillInfo` carries; P1 replaces WHEN it is registered.

#### Task P4.7a: `SkillTemplate` — CC/pi argument grammar + `` !`cmd` `` with a consent-gated shell

**Files:**
- Modify: `src/extension/template.rs` (whole `render` path; the `@file` machinery stays)
- Test: `src/extension/template.rs` `mod tests`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  ```rust
  // src/extension/template.rs
  #[async_trait::async_trait]
  pub trait InlineShell: Send + Sync {
      /// Run one `` !`cmd` `` body. `Ok(stdout)` is substituted; `Err(placeholder)`
      /// is substituted VERBATIM (consent withheld, timeout, non-zero exit) so
      /// the model sees that something was withheld, not a silent blank.
      async fn run(&self, cmd: &str) -> Result<String, String>;
  }
  pub struct TemplateCtx<'a> { pub shell: Option<&'a dyn InlineShell> }
  impl SkillTemplate {
      pub async fn render(&self, arguments: &str, ctx: &TemplateCtx<'_>) -> ExtensionResult<String>;
  }
  pub fn split_arguments(args: &str) -> Vec<String>;   // whitespace split, no quoting
  ```
  Contract delta: the contract sketched `TemplateCtx { cwd, file_reader, shell }`. The real `SkillTemplate` already owns its base dir and a jailed async file reader (`resolve_path` / `validate_path_security` / `read_file`, `template.rs:120-215`) — a second `file_reader` closure would be a second answer to "how is `@file` read". `cwd` belongs to the shell runner. Recorded below.

Grammar (CC = pi, `scan-pi.md` §4.2 / `scan-cc-plugin-format.md` §2), applied in this order so `` !`gh pr view $1` `` sees its argument: `${N:-default}` → `$N` → `$@` and `$ARGUMENTS` → `` !`cmd` `` → `@./file`. `$N` is 1-based over `split_arguments` (whitespace; quoting is not interpreted — DEVIATION #20 note). An out-of-range `$N` with no default becomes the empty string (CC behaviour).

Existing `render` (`template.rs:76-88`):

```rust
    pub async fn render(&self, arguments: &str) -> ExtensionResult<String> {
        // 1. Replace $ARGUMENTS
        let mut result = self.content.replace("$ARGUMENTS", arguments);

        // 2. Expand file references
        result = self.expand_file_refs(&result).await?;

        Ok(result)
    }
```

- [ ] **Step 1: Write the failing tests** (replace the two `$ARGUMENTS` tests' calls with the new signature and add these)

```rust
    struct NoShell;
    fn no_shell() -> TemplateCtx<'static> {
        TemplateCtx { shell: None }
    }

    #[tokio::test]
    async fn positional_and_default_arguments() {
        let t = SkillTemplate::new("pr=$1 who=${2:-nobody} all=[$ARGUMENTS] again=[$@]", Path::new("/x/cmd.md"));
        assert_eq!(
            t.render("123", &no_shell()).await.unwrap(),
            "pr=123 who=nobody all=[123] again=[123]"
        );
        assert_eq!(
            t.render("123 alice", &no_shell()).await.unwrap(),
            "pr=123 who=alice all=[123 alice] again=[123 alice]"
        );
        // `$10` is the tenth argument, not `$1` followed by `0`.
        let t = SkillTemplate::new("[$10][$1]", Path::new("/x/cmd.md"));
        assert_eq!(t.render("a b c d e f g h i j", &no_shell()).await.unwrap(), "[j][a]");
        // Out of range with no default → empty, not the literal.
        let t = SkillTemplate::new("[$3]", Path::new("/x/cmd.md"));
        assert_eq!(t.render("a", &no_shell()).await.unwrap(), "[]");
    }

    struct RecordingShell(std::sync::Mutex<Vec<String>>, Result<String, String>);
    #[async_trait::async_trait]
    impl InlineShell for RecordingShell {
        async fn run(&self, cmd: &str) -> Result<String, String> {
            self.0.lock().unwrap().push(cmd.to_string());
            self.1.clone()
        }
    }

    #[tokio::test]
    async fn inline_shell_runs_after_argument_substitution() {
        let shell = RecordingShell(Default::default(), Ok("main\n".into()));
        let t = SkillTemplate::new("Branch: !`git branch --show-current $1`.", Path::new("/x/cmd.md"));
        let out = t.render("--verbose", &TemplateCtx { shell: Some(&shell) }).await.unwrap();
        assert_eq!(out, "Branch: main.");
        assert_eq!(shell.0.lock().unwrap().as_slice(), ["git branch --show-current --verbose"]);
    }

    #[tokio::test]
    async fn withheld_shell_leaves_the_placeholder_in_place() {
        // Consent not given (or the command failed): the placeholder is what
        // the model reads, so a withheld expansion is visible, never blank.
        let shell = RecordingShell(Default::default(), Err("[withheld: pending approval]".into()));
        let t = SkillTemplate::new("Files: !`git diff --name-only`", Path::new("/x/cmd.md"));
        let out = t.render("", &TemplateCtx { shell: Some(&shell) }).await.unwrap();
        assert_eq!(out, "Files: [withheld: pending approval]");
    }

    #[tokio::test]
    async fn no_shell_means_every_inline_command_is_withheld() {
        let t = SkillTemplate::new("!`whoami` done", Path::new("/x/cmd.md"));
        let out = t.render("", &no_shell()).await.unwrap();
        assert!(out.starts_with("[!`whoami` not run"), "{out}");
        assert!(out.ends_with(" done"));
    }

    #[test]
    fn split_arguments_is_whitespace_only() {
        assert_eq!(split_arguments("  a   b\tc "), ["a", "b", "c"]);
        assert_eq!(split_arguments("\"a b\""), ["\"a", "b\""], "quoting is not interpreted (documented)");
        assert!(split_arguments("").is_empty());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib extension::template -- --nocapture`
Expected: compile errors (`TemplateCtx`, `InlineShell`, `split_arguments` missing; `render` takes one argument).

- [ ] **Step 3: Write the implementation**

Replace `render` and add the grammar (keep `expand_file_refs`, `resolve_path`, `validate_path_security`, `read_file` unchanged):

```rust
/// Runs one `` !`cmd` `` body for [`SkillTemplate::render`].
///
/// The template knows nothing about consent, cwd or timeouts — the caller
/// (the gateway's slash-command seam) owns those and hands in an
/// implementation. `Err(text)` is substituted verbatim: a withheld or failed
/// expansion must be visible to the model as such, never a silent blank.
#[async_trait::async_trait]
pub trait InlineShell: Send + Sync {
    async fn run(&self, cmd: &str) -> Result<String, String>;
}

/// What a render may reach beyond its own file: the shell for `` !`cmd` ``,
/// or `None` (every inline command is then withheld with a placeholder).
pub struct TemplateCtx<'a> {
    pub shell: Option<&'a dyn InlineShell>,
}

/// Positional arguments for `$1 … $N`: whitespace-split, quoting NOT
/// interpreted (Claude Code and pi do the same; a shell-quoted argument
/// reaches `$1` with its quotes).
#[must_use]
pub fn split_arguments(args: &str) -> Vec<String> {
    args.split_whitespace().map(str::to_string).collect()
}

/// `${N:-default}` and `$N` (1-based; out of range → empty string).
fn positional_regex() -> &'static Regex {
    static RE: OnceCell<Regex> = OnceCell::new();
    RE.get_or_init(|| Regex::new(r"\$\{(\d+):-([^}]*)\}|\$(\d+)").expect("literal regex"))
}

/// `` !`cmd` `` — a backtick-fenced shell body after a bang.
fn inline_shell_regex() -> &'static Regex {
    static RE: OnceCell<Regex> = OnceCell::new();
    RE.get_or_init(|| Regex::new(r"!`([^`]+)`").expect("literal regex"))
}

impl SkillTemplate {
    /// Render with the Claude Code / pi prompt-template grammar, in this order:
    /// `${N:-default}` and `$N` → `$@` and `$ARGUMENTS` → `` !`cmd` `` →
    /// `@./file`. Arguments are substituted BEFORE the shell runs so a
    /// command may use them (`` !`gh pr view $1` ``).
    pub async fn render(&self, arguments: &str, ctx: &TemplateCtx<'_>) -> ExtensionResult<String> {
        let positional = split_arguments(arguments);
        let mut result = positional_regex()
            .replace_all(&self.content, |cap: &regex::Captures<'_>| {
                let (index, default) = match (cap.get(1), cap.get(3)) {
                    (Some(n), _) => (n.as_str(), cap.get(2).map_or("", |d| d.as_str())),
                    (None, Some(n)) => (n.as_str(), ""),
                    _ => return String::new(),
                };
                index
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| i.checked_sub(1))
                    .and_then(|i| positional.get(i))
                    .map_or_else(|| default.to_string(), Clone::clone)
            })
            .into_owned();
        result = result.replace("$ARGUMENTS", arguments).replace("$@", arguments);
        result = self.expand_inline_shell(&result, ctx).await;
        self.expand_file_refs(&result).await
    }

    /// Substitute every `` !`cmd` ``. Sequential and in document order — the
    /// commands may depend on each other's side effects, and CC runs them
    /// that way.
    async fn expand_inline_shell(&self, content: &str, ctx: &TemplateCtx<'_>) -> String {
        let mut out = String::with_capacity(content.len());
        let mut last = 0;
        for cap in inline_shell_regex().captures_iter(content) {
            let whole = cap.get(0).expect("group 0");
            let cmd = cap.get(1).expect("group 1").as_str().trim();
            out.push_str(&content[last..whole.start()]);
            let text = match ctx.shell {
                Some(shell) => match shell.run(cmd).await {
                    Ok(stdout) => stdout.trim_end().to_string(),
                    Err(placeholder) => placeholder,
                },
                None => format!("[!`{cmd}` not run: no shell available for inline commands]"),
            };
            out.push_str(&text);
            last = whole.end();
        }
        out.push_str(&content[last..]);
        out
    }
}
```

(`SkillRegistration::with_arguments` at `registry/types.rs:244-248` is a second `$ARGUMENTS` replacer with zero callers — `rg 'with_arguments\(' src` finds only `HookContext::with_arguments` at `dispatch.rs:1450`; delete it in this task. `mod template;` is private (`extension/mod.rs:51`) and only `SkillTemplate` is re-exported (`:64`): widen that line to `pub use template::{split_arguments, InlineShell, SkillTemplate, TemplateCtx};` so P4.7c can name the trait.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib extension::template -- --nocapture`
Expected: PASS (all pre-existing `@file` tests included; `test_arguments_substitution` / `test_multiple_arguments` / `test_combined_template` updated to `render(.., &no_shell())`).

- [ ] **Step 5: Commit**

```bash
git add src/extension/template.rs src/extension/registry/types.rs
git commit -m "template: CC/pi argument grammar and consent-gated inline shell for command bodies

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

#### Task P4.7b: Parse command frontmatter (`argument-hint`, `allowed-tools`, `model`, `disable-model-invocation`) and register it

**Files:**
- Modify: `src/extension/manifest/parsers.rs:50-60` (`SkillFm`), `:315-345` (`parse_skill_registration`)
- Modify: `src/extension/registry/types.rs:183-225` (`SkillRegistration` gains three fields)
- Modify: `src/skill/compat.rs:17-40` (`SkillInfo.argument_hint`), `src/tool_metadata/registry/registration.rs:266-276` (`with_usage` reads it)
- Modify: `src/extension/slash_effect.rs` — **P1.7's** `plugin_command_skill_info` (the singular builder, "THE builder of a plugin command's `SkillInfo`"; `plugin_command_skill_infos` maps over it and is untouched; the 3ddc1f2e7 site `tool_catalog_init.rs:214-247` is deleted by P1.10 — R4.4) + the comment in `register_slash_commands_effect` that says `allowed_tools` is always `None` + P1.7's test `plugin_command_skill_info_projects_the_qualified_name_and_nothing_else`
- Test: `src/extension/manifest/parsers.rs` `mod tests`, `src/extension/slash_effect.rs` `mod tests`

**Interfaces:**
- Produces:
  ```rust
  // src/extension/registry/types.rs — on SkillRegistration
  #[serde(default, skip_serializing_if = "Option::is_none")] pub argument_hint: Option<String>,
  /// CC `allowed-tools`, ALREADY normalised to Aleph names (P4.5 table, restrict mode).
  #[serde(default, skip_serializing_if = "Option::is_none")] pub allowed_tools: Option<Vec<String>>,
  #[serde(default, skip_serializing_if = "Option::is_none")] pub model: Option<String>,
  // src/skill/compat.rs — on SkillInfo
  #[serde(default, skip_serializing_if = "Option::is_none")] pub argument_hint: Option<String>,
  ```

Current `SkillFm` (`parsers.rs:50-60`) has `name / description / triggers / category` only. The `SkillInfo` projection for plugin commands hard-codes `allowed_tools: None` — at 3ddc1f2e7 in `tool_catalog_init.rs:225-240`, after P1 in `slash_effect::plugin_command_skill_info` (plan-P1 P1.7, quoted in Step 3; its own doc says "Later rounds add fields HERE (… `argument_hint` / `allowed_tools` / `model` from the command's frontmatter), never at a second construction site"), which is the site this task edits.

- [ ] **Step 1: Write the failing tests**

`src/extension/manifest/parsers.rs` `mod tests` (the file has a test module with `parse_commands_dir` fixtures; add):

```rust
    #[test]
    fn command_frontmatter_fields_reach_the_registration() {
        let dir = tempfile::tempdir().unwrap();
        let cmds = dir.path().join("commands");
        std::fs::create_dir_all(&cmds).unwrap();
        std::fs::write(
            cmds.join("review.md"),
            "---\n\
             description: Code review a pull request\n\
             argument-hint: \"[pr-number] [priority]\"\n\
             allowed-tools: Bash(gh pr view:*), Read, Grep\n\
             model: sonnet\n\
             disable-model-invocation: true\n\
             ---\n\
             Review PR $1.\n",
        )
        .unwrap();
        let caps = parse_commands_dir(dir.path(), "commands", "plug").unwrap();
        let CapabilityDeclaration::Skill(reg) = &caps[0] else { panic!("skill") };
        assert_eq!(reg.skill_type, crate::extension::types::SkillType::Command);
        assert_eq!(reg.argument_hint.as_deref(), Some("[pr-number] [priority]"));
        // Normalised through the P4.5 table in RESTRICT mode: scoped Bash folds
        // to bare `bash`; CC names become Aleph names.
        assert_eq!(
            reg.allowed_tools.as_deref(),
            Some(&["bash".to_string(), "file_read".to_string(), "grep".to_string()][..])
        );
        assert_eq!(reg.model.as_deref(), Some("sonnet"));
        assert!(reg.disable_model_invocation);
        assert_eq!(reg.content, "Review PR $1.");
    }

    #[test]
    fn a_command_with_no_extra_frontmatter_declares_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cmds = dir.path().join("commands");
        std::fs::create_dir_all(&cmds).unwrap();
        std::fs::write(cmds.join("hi.md"), "Say hi to $ARGUMENTS.\n").unwrap();
        let caps = parse_commands_dir(dir.path(), "commands", "plug").unwrap();
        let CapabilityDeclaration::Skill(reg) = &caps[0] else { panic!("skill") };
        assert!(reg.argument_hint.is_none() && reg.allowed_tools.is_none() && reg.model.is_none());
    }

    #[test]
    fn allowed_tools_array_form_parses_too() {
        // `create-plugin.md` on this machine uses the YAML array form.
        let dir = tempfile::tempdir().unwrap();
        let cmds = dir.path().join("commands");
        std::fs::create_dir_all(&cmds).unwrap();
        std::fs::write(
            cmds.join("c.md"),
            "---\nallowed-tools:\n  [\"Read\",\"Write\",\"Bash\",\"TodoWrite\"]\n---\nbody\n",
        )
        .unwrap();
        let caps = parse_commands_dir(dir.path(), "commands", "plug").unwrap();
        let CapabilityDeclaration::Skill(reg) = &caps[0] else { panic!("skill") };
        // `TodoWrite` has no Aleph counterpart and is dropped (documented gap),
        // never passed through as a name the registry would reject the whole
        // command over.
        assert_eq!(
            reg.allowed_tools.as_deref(),
            Some(&["file_read".to_string(), "file_write".to_string(), "bash".to_string()][..])
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib extension::manifest::parsers -- --nocapture`
Expected: compile error — no `argument_hint` / `allowed_tools` / `model` on `SkillRegistration`.

- [ ] **Step 3: Write the implementation**

`parsers.rs:50-60` `SkillFm`:

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct SkillFm {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    triggers: Option<Vec<String>>,
    #[serde(default)]
    category: Option<String>,
    /// Claude Code command frontmatter. `allowed-tools` is raw YAML because
    /// upstream writes both the comma-string and the array form
    /// (`skill::frontmatter::normalize_allowed_tools` reads both).
    #[serde(default)]
    argument_hint: Option<String>,
    #[serde(default)]
    allowed_tools: Option<crate::yaml::Value>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    disable_model_invocation: Option<bool>,
}
```

(`rename_all = "kebab-case"` turns `argument_hint` into the wire key `argument-hint`; `name`/`description`/`triggers`/`category` are single words and unchanged.)

`parse_skill_registration` (`:315-345`) — build the registration with the new fields:

```rust
    let name = fm.name.unwrap_or_else(|| default_name.to_string());
    // CC → Aleph names, RESTRICT mode (a command's `allowed-tools` narrows
    // the turn's tool surface; scoped `Bash(...)` folds to bare `bash`). A
    // CC name with no Aleph counterpart is dropped with a warn rather than
    // forwarded — `register_skills` refuses the whole command on one unknown
    // name, and losing the slash command over `TodoWrite` is the worse answer.
    let allowed_tools = crate::skill::frontmatter::normalize_allowed_tools(fm.allowed_tools.as_ref(), &name)
        .map(|names| {
            names
                .iter()
                .filter_map(|n| {
                    let mapped = crate::extension::hooks::normalize_cc_tool_entry(n, true);
                    if mapped.is_none() {
                        warn!(command = %name, entry = %n, "allowed-tools entry has no Aleph tool; dropped");
                    }
                    mapped
                })
                .collect::<Vec<_>>()
        });
    Ok(CapabilityDeclaration::Skill(SkillRegistration {
        name,
        description: fm.description.unwrap_or_default(),
        content: body,
        triggers: fm.triggers.unwrap_or_default(),
        category: fm.category,
        plugin_id: plugin_id.to_string(),
        skill_type,
        source_path: md_path.to_path_buf(),
        argument_hint: fm.argument_hint,
        allowed_tools,
        model: fm.model,
        disable_model_invocation: fm.disable_model_invocation.unwrap_or(false),
        ..Default::default()
    }))
```

`registry/types.rs` — three fields on `SkillRegistration` (docs as in Interfaces). `src/skill/compat.rs` — `SkillInfo.argument_hint: Option<String>` (and `argument_hint: None` in `From<SkillManifest>`). `registration.rs:275`:

```rust
            .with_usage(match skill.argument_hint.as_deref() {
                Some(hint) => format!("/{} {hint}", skill.id),
                None => format!("/{} [input]", skill.id),
            })
```

`src/extension/slash_effect.rs` — P1.7's builder (quoted from plan-P1 P1.7, the shape it leaves behind):

```rust
pub(crate) fn plugin_command_skill_info(cmd: &SkillRegistration) -> SkillInfo {
    SkillInfo {
        id: cmd.qualified_name(),
        name: cmd.name.clone(),
        description: cmd.description.clone(),
        // Plugin commands have no SkillManifest behind them; System is the
        // manifest default and matches every other slash command.
        scope: crate::domain::skill::PromptScope::System,
        version: None,
        // No `allowed-tools:` to project: `None` keeps the agent's full tool
        // surface, which is what plugin commands have always done.
        allowed_tools: None,
    }
}
```

becomes (project the registration instead of constants; `plugin_command_skill_infos` maps over this and needs no change):

```rust
pub(crate) fn plugin_command_skill_info(cmd: &SkillRegistration) -> SkillInfo {
    SkillInfo {
        id: cmd.qualified_name(),
        name: cmd.name.clone(),
        description: cmd.description.clone(),
        // Plugin commands have no SkillManifest behind them; System is the
        // manifest default and matches every other slash command.
        scope: crate::domain::skill::PromptScope::System,
        version: None,
        // The command's own `allowed-tools`, already Aleph names
        // (`manifest/parsers.rs`, RESTRICT mode). `None` keeps the full surface.
        allowed_tools: cmd.allowed_tools.clone(),
        argument_hint: cmd.argument_hint.clone(),
    }
}
```

Two P1.7 texts become false with this and are corrected in the same commit (判据 §1): the doc comment on `register_slash_commands_effect` ("A skill the catalog refuses (unknown `allowed-tools:` name — impossible here, `allowed_tools` is always `None`) …") → "A command the catalog refuses (an `allowed-tools:` name the registry does not know — every name has been through the CC alias table, so this is a genuine unknown) is simply not in the id list the disposer removes, and `register_skills` has already warned by name."; and the test `plugin_command_skill_info_projects_the_qualified_name_and_nothing_else` — keep its qualified-name / scope / version assertions, replace the `allowed_tools.is_none()` line with a second fixture:

```rust
        // The frontmatter half (P4.7b): declared `allowed-tools` (already
        // Aleph names) and `argument-hint` ride onto the SkillInfo.
        let mut with_fm = cmd("qa-plug", "review");
        with_fm.allowed_tools = Some(vec!["grep".into(), "bash".into()]);
        with_fm.argument_hint = Some("[pr-number]".into());
        let info = plugin_command_skill_info(&with_fm);
        assert_eq!(info.allowed_tools.as_deref(), Some(&["grep".to_string(), "bash".to_string()][..]));
        assert_eq!(info.argument_hint.as_deref(), Some("[pr-number]"));
        // …and a command that declares nothing keeps the full surface.
        assert!(plugin_command_skill_info(&cmd("qa-plug", "hello")).allowed_tools.is_none());
```

(rename it `plugin_command_skill_info_projects_the_qualified_name_and_the_frontmatter`).

Also update the doc on `SkillInfo.allowed_tools` (`compat.rs:36-38`: "Plugin commands … always project `None`" is no longer true).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib extension::manifest -- --nocapture && cargo test -p alephcore --lib extension::slash_effect -- --nocapture && cargo test -p alephcore --lib skill:: -- --nocapture && cargo test -p alephcore --lib tool_metadata -- --nocapture && cargo test -p alephcore --bins`
Expected: PASS (P1.13's round-trip fixture still mounts/unmounts its command: the projection changed shape, not identity).

- [ ] **Step 5: Commit**

```bash
git add src/extension/manifest/parsers.rs src/extension/registry/types.rs src/skill/compat.rs src/tool_metadata/registry/registration.rs src/extension/slash_effect.rs
git commit -m "extension: parse CC command frontmatter (argument-hint, allowed-tools, model)

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

#### Task P4.7c: `/cmd args` renders the body and injects it as this turn's transient user content (+ `model` pin)

**Files:**
- Create: `src/gateway/execution_engine/slash_command_body.rs`
- Modify: `src/gateway/execution_engine/mod.rs` (`mod slash_command_body;` + `pub const SLASH_COMMAND_BODY_KEY`)
- Modify: `src/gateway/execution_engine/execute.rs:764-782` (the `type == "skill"` block)
- Modify: `src/gateway/execution_engine/run_loop/inner.rs:396-397` (push the block first)
- Test: `src/gateway/execution_engine/slash_command_body.rs` `mod tests`

**Interfaces:**
- Consumes: `SkillTemplate::render` + `InlineShell` (P4.7a), `SkillRegistration.{content,source_path,model,plugin_id,skill_type}` (P4.7b), `ShellHookConsent::{shared,is_approved,record_pending}` (`hooks/consent.rs:128,166,211`), `read_capped` (`hooks/executor.rs:36`, `pub(crate) use` in `hooks/mod.rs:53`), `ModelOverride::Raw` (`gateway/model_override.rs`).
- Produces:
  ```rust
  // src/gateway/execution_engine/mod.rs
  /// Request-metadata key: the rendered `<command>` block for a `/cmd` turn.
  /// Written by `slash_command_body::stamp`, read once by the run loop.
  pub const SLASH_COMMAND_BODY_KEY: &str = "slash_command_body";
  // src/gateway/execution_engine/slash_command_body.rs
  pub(super) struct CommandTurn { pub block: String, pub model: Option<String> }
  pub(super) async fn resolve_command_turn(mode: &serde_json::Value, cwd: Option<&std::path::Path>) -> Option<CommandTurn>;
  pub(super) fn wrap_block(qualified: &str, plugin_id: &str, rendered: &str) -> String;
  pub(super) async fn render_registration(reg: &SkillRegistration, args: &str, shell: &dyn InlineShell) -> Result<String, ExtensionError>;
  pub(super) struct ConsentedShell { plugin_id: String, cwd: PathBuf, consent: Arc<ShellHookConsent> }
  ```

Consent reuse, exactly as command hooks do it (`executor.rs:517-538`): the key is `(plugin_id, command text)`; un-approved → `record_pending(plugin_id, cmd, "SlashCommand")` + a placeholder; `aleph hooks list` shows it and `aleph hooks test <fp>` approves it, the same flow as a plugin's `hooks.json` command. Approved → `sh -c` (`cmd /C` on Windows) in `cwd`, 30 s timeout, stdout capped by `read_capped` at 64 KiB, non-zero exit → placeholder with the exit code.

- [ ] **Step 1: Write the failing tests**

`src/gateway/execution_engine/slash_command_body.rs` `mod tests`:

```rust
    fn command(body: &str) -> SkillRegistration {
        SkillRegistration {
            name: "greet".into(),
            plugin_id: "plug".into(),
            skill_type: crate::extension::types::SkillType::Command,
            content: body.into(),
            source_path: std::path::PathBuf::from("/nowhere/commands/greet.md"),
            ..Default::default()
        }
    }

    struct Never;
    #[async_trait::async_trait]
    impl InlineShell for Never {
        async fn run(&self, cmd: &str) -> Result<String, String> {
            Err(format!("[!`{cmd}` withheld]"))
        }
    }

    #[tokio::test]
    async fn the_rendered_body_is_wrapped_as_a_command_block() {
        let reg = command("Say hello to $1 and mention QA_MARKER_$1.");
        let rendered = render_registration(&reg, "World", &Never).await.unwrap();
        assert_eq!(rendered, "Say hello to World and mention QA_MARKER_World.");
        let block = wrap_block("plug:greet", "plug", &rendered);
        assert!(block.starts_with("<command name=\"greet\" plugin=\"plug\" invoked=\"/plug:greet\">"), "{block}");
        assert!(block.ends_with("</command>"));
        assert!(block.contains("QA_MARKER_World"));
    }

    #[tokio::test]
    async fn a_skill_mode_that_names_a_real_skill_is_not_a_command_turn() {
        // `/my-skill` resolves through the same `type: "skill"` envelope; only
        // a `SkillType::Command` registration renders a body.
        let mode = serde_json::json!({"type":"skill","skill_id":"not-registered","args":""});
        assert!(resolve_command_turn(&mode, None).await.is_none());
    }

    #[test]
    fn consent_is_asked_under_the_plugin_id_and_withheld_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let consent = std::sync::Arc::new(crate::extension::hooks::ShellHookConsent::with_path(dir.path().join("allow.json")));
        let shell = ConsentedShell::new("plug", dir.path().to_path_buf(), consent.clone());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let out = rt.block_on(shell.run("echo hi"));
        assert!(out.is_err(), "unapproved must be withheld: {out:?}");
        assert!(out.unwrap_err().contains("aleph hooks"), "placeholder names the remedy");
        // Recorded as pending under the plugin id, like a hooks.json command.
        let entry = &consent.entries()[0];
        assert_eq!(entry.plugin_name, "plug");
        assert_eq!(entry.command, "echo hi");
        assert_eq!(entry.event, "SlashCommand");
        // Approve → runs.
        consent.approve(&entry.fingerprint.clone()).unwrap();
        let out = rt.block_on(shell.run("echo hi")).unwrap();
        assert_eq!(out.trim(), "hi");
    }

    #[test]
    fn a_declared_model_becomes_a_raw_override_only_when_the_request_has_none() {
        use crate::gateway::model_override::ModelOverride;
        assert_eq!(
            model_override_for(None, Some("claude-sonnet-5")),
            Some(ModelOverride::Raw { model: "claude-sonnet-5".into() })
        );
        let user = ModelOverride::Raw { model: "gpt-5".into() };
        assert_eq!(model_override_for(Some(&user), Some("claude-sonnet-5")), Some(user.clone()), "the user's pick wins");
        // CC's bare aliases are not model ids Aleph can route (no catalog
        // alias facility) — ignored, documented.
        assert_eq!(model_override_for(None, Some("sonnet")), None);
        assert_eq!(model_override_for(None, None), None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib execution_engine::slash_command_body -- --nocapture`
Expected: compile error (module missing).

- [ ] **Step 3: Write the implementation**

`src/gateway/execution_engine/slash_command_body.rs`:

```rust
//! `/cmd args` → the command's rendered markdown body, as this turn's
//! transient user content.
//!
//! A CC plugin's `commands/<name>.md` body is "literally Claude's
//! instructions when invoked" (plugin-dev skill, verbatim). Until this module
//! nothing on the `/cmd` path put that text in front of the model: the slash
//! resolver fell through to the agent loop with the raw `/cmd args` and the
//! body sat in `SkillRegistration.content`, parsed and never read.
//!
//! The body rides the run loop's transient channel (`transient_blocks` →
//! `HarnessDeps::recall_context`): delivered to the model every Think, never
//! persisted, so the stored user turn — and the session title derived from it
//! — stays the raw input. Claude Code persists the expansion instead; the
//! difference is recorded in PLUGIN_SYSTEM.md.

use std::path::{Path, PathBuf};

use crate::extension::hooks::{read_capped, ShellHookConsent};
use crate::extension::{InlineShell, SkillTemplate, TemplateCtx};
use crate::extension::{ExtensionError, SkillRegistration};
use crate::gateway::model_override::ModelOverride;
use crate::sync_primitives::Arc;

/// Bound on one `` !`cmd` `` expansion. Well under the hook ceiling: this
/// runs BEFORE the turn's first Think, and a command that takes longer than
/// this is a command that belongs in the body as an instruction to the
/// model, not in an inline expansion.
const INLINE_SHELL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Same cap as a hook's stdout — an expansion is prompt text.
const INLINE_SHELL_OUTPUT_CAP: u64 = 64 * 1024;
/// Consent-registry event label for inline command expansions.
const CONSENT_EVENT: &str = "SlashCommand";

/// What a resolved `/cmd` contributes to the turn.
pub(super) struct CommandTurn {
    /// The `<command …>…</command>` block for `transient_blocks`.
    pub block: String,
    /// The command's `model:` frontmatter, if any (see [`model_override_for`]).
    pub model: Option<String>,
}

/// Resolve the slash-mode JSON to a command turn, or `None` when the mode is
/// not a `type: "skill"` envelope naming a `SkillType::Command` registration.
pub(super) async fn resolve_command_turn(
    mode: &serde_json::Value,
    cwd: Option<&Path>,
) -> Option<CommandTurn> {
    if mode.get("type").and_then(|v| v.as_str()) != Some("skill") {
        return None;
    }
    let skill_id = mode.get("skill_id").and_then(|v| v.as_str())?;
    let args = mode.get("args").and_then(|v| v.as_str()).unwrap_or("");
    let manager = crate::extension::try_extension_manager()?;
    let reg = {
        let registry = manager.get_plugin_registry().await;
        registry
            .get_skill(skill_id)
            .filter(|s| s.skill_type == crate::extension::types::SkillType::Command)
            .cloned()?
    };
    let cwd = cwd
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| reg.base_dir());
    let shell = ConsentedShell::new(&reg.plugin_id, cwd, ShellHookConsent::shared());
    let rendered = match render_registration(&reg, args, &shell).await {
        Ok(text) => text,
        Err(e) => {
            tracing::warn!(command = %skill_id, error = %e, "command body failed to render; sending the raw input");
            return None;
        }
    };
    Some(CommandTurn {
        block: wrap_block(skill_id, &reg.plugin_id, &rendered),
        model: reg.model.clone(),
    })
}

/// Render one registration's body with the CC grammar. `base_dir` for
/// `@./file` is the command file's directory (`SkillRegistration::base_dir`).
pub(super) async fn render_registration(
    reg: &SkillRegistration,
    args: &str,
    shell: &dyn InlineShell,
) -> Result<String, ExtensionError> {
    SkillTemplate::with_base_dir(&reg.content, reg.base_dir())
        .render(args, &TemplateCtx { shell: Some(shell) })
        .await
}

/// The block the model reads. Named so the model can tell "the user invoked a
/// command" from "the user typed this"; the raw `/…` text is the persisted turn.
pub(super) fn wrap_block(qualified: &str, plugin_id: &str, rendered: &str) -> String {
    let name = qualified.rsplit(':').next().unwrap_or(qualified);
    format!(
        "<command name=\"{name}\" plugin=\"{plugin_id}\" invoked=\"/{qualified}\">\n{}\n</command>",
        rendered.trim()
    )
}

/// A command's `model:` as this turn's override — only when the request
/// carries none (the user's composer pick wins) and only for a model id the
/// resolver can route. Claude Code's bare aliases (`sonnet` / `opus` /
/// `haiku`) are not ids in any provider's catalog and Aleph has no alias
/// facility; they are ignored (documented DEVIATION #18).
pub(super) fn model_override_for(
    requested: Option<&ModelOverride>,
    declared: Option<&str>,
) -> Option<ModelOverride> {
    if let Some(user) = requested {
        return Some(user.clone());
    }
    let model = declared?.trim();
    if model.is_empty() || matches!(model, "sonnet" | "opus" | "haiku" | "inherit") {
        return None;
    }
    Some(ModelOverride::Raw { model: model.to_string() })
}

/// The `` !`cmd` `` runner: the SAME consent registry and the same
/// `(plugin, command)` key as a plugin's `hooks.json` shell command
/// (`HookExecutor::execute_command`), so `aleph hooks list` / `aleph hooks
/// test` are the one review surface for every shell a plugin can reach.
pub(super) struct ConsentedShell {
    plugin_id: String,
    cwd: PathBuf,
    consent: Arc<ShellHookConsent>,
}

impl ConsentedShell {
    pub(super) fn new(plugin_id: &str, cwd: PathBuf, consent: Arc<ShellHookConsent>) -> Self {
        Self { plugin_id: plugin_id.to_string(), cwd, consent }
    }
}

#[async_trait::async_trait]
impl InlineShell for ConsentedShell {
    async fn run(&self, cmd: &str) -> Result<String, String> {
        if !self.consent.is_approved(&self.plugin_id, cmd) {
            self.consent.record_pending(&self.plugin_id, cmd, CONSENT_EVENT);
            tracing::warn!(plugin = %self.plugin_id, command = %cmd, "inline command not approved — withheld; review with `aleph hooks list`");
            return Err(format!(
                "[!`{cmd}` not run: pending operator approval — `aleph hooks list` / `aleph hooks test`]"
            ));
        }
        let mut command = if cfg!(windows) {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/C", cmd]);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", cmd]);
            c
        };
        command
            .current_dir(&self.cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        use crate::utils::no_window::NoWindow;
        let run = async {
            let mut child = command.no_window().spawn().map_err(|e| format!("[!`{cmd}` failed to start: {e}]"))?;
            let stdout = child.stdout.take();
            let (buf, truncated) = match stdout {
                Some(h) => read_capped(h, INLINE_SHELL_OUTPUT_CAP).await,
                None => (Vec::new(), false),
            };
            let status = child.wait().await.map_err(|e| format!("[!`{cmd}` failed: {e}]"))?;
            if !status.success() {
                return Err(format!("[!`{cmd}` exited {}]", status.code().map_or("by signal".to_string(), |c| c.to_string())));
            }
            let mut text = String::from_utf8_lossy(&buf).into_owned();
            if truncated {
                text.push_str("\n...[truncated: output exceeds 64 KiB cap]");
            }
            Ok(text)
        };
        match tokio::time::timeout(INLINE_SHELL_TIMEOUT, run).await {
            Ok(r) => r,
            Err(_) => Err(format!("[!`{cmd}` timed out after {}s]", INLINE_SHELL_TIMEOUT.as_secs())),
        }
    }
}
```

`src/gateway/execution_engine/mod.rs`: `mod slash_command_body;` and the `SLASH_COMMAND_BODY_KEY` const (doc as in Interfaces).

`execute.rs:776-782` — extend the existing block:

```rust
        if let Some(mode_json) = request.metadata.get(SLASH_COMMAND_MODE_KEY).cloned() {
            if let Ok(mode) = serde_json::from_str::<serde_json::Value>(&mode_json) {
                if mode.get("type").and_then(|v| v.as_str()) == Some("skill") {
                    super::slash_skill_scope::stamp_from_mode(&mut request.metadata, &mode);
                    // A plugin COMMAND (not a skill): render its body now — the
                    // `!` expansions and `@file` reads happen once, here — and
                    // hand the block to the run loop through metadata. The
                    // command's `model:` becomes this turn's override only when
                    // the composer sent none.
                    if let Some(turn) = super::slash_command_body::resolve_command_turn(
                        &mode,
                        request.workspace_override.as_deref(),
                    )
                    .await
                    {
                        request.metadata.insert(super::SLASH_COMMAND_BODY_KEY.to_string(), turn.block);
                        request.model_override = super::slash_command_body::model_override_for(
                            request.model_override.as_ref(),
                            turn.model.as_deref(),
                        );
                    }
                }
            }
        }
```

(The comment block at `execute.rs:765-775` saying "Nothing here injects skill text into the prompt, and no code anywhere does" becomes false for COMMANDS; rewrite it to: skills still reach the model only through `<available_skills>` + `skill_read`; a plugin command's body is rendered here.)

`inner.rs:396-397` — right after `let mut transient_blocks: Vec<String> = Vec::new();`:

```rust
        // A `/cmd` turn: the command's rendered body, first — it IS the
        // user's instruction for this turn; the reminders below annotate it.
        if let Some(block) = request.metadata.get(super::super::SLASH_COMMAND_BODY_KEY) {
            transient_blocks.push(block.clone());
        }
```

(and delete the stale sentence in the `slash_command.rs:249-262` `"skill"` arm comment: "nothing on this path injects skill text into the prompt" → "a plugin COMMAND's body is rendered in `execute.rs` and rides `transient_blocks`; a SKILL's body still reaches the model only via `skill_read`".)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib execution_engine::slash_command_body -- --nocapture && cargo test -p alephcore --lib execution_engine -- --nocapture`
Expected: PASS. `run_loop/mod.rs::every_pre_seed_hook_exit_journals_the_stop` and `the_twin_hook_seams_fire_before_anything_seeds_the_turn` still green (the new read of metadata is not a seed writer).

- [ ] **Step 5: Commit**

```bash
git add src/gateway/execution_engine/slash_command_body.rs src/gateway/execution_engine/mod.rs src/gateway/execution_engine/execute.rs src/gateway/execution_engine/run_loop/inner.rs src/gateway/execution_engine/slash_command.rs
git commit -m "gateway: render a plugin command's body on /cmd and inject it as the turn's content

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

#### Task P4.7d: CUT `plugins.executeCommand` and the WASM "command handler" chain

Zero clients (`rpc_census.sh`; `rg -n 'executeCommand|execute_plugin_command' interfaces shared qa` → 0 hits at 3ddc1f2e7), and semantically wrong since the `CommandRegistration → SkillRegistration{Command}` fold: `handle_execute_command` looks up a markdown body and passes it to `PluginLoader::execute_command` as a WASM export NAME (`runtime.rs:82-116`). P5 owns `plugins.{load,unload}`; this task owns only the `executeCommand` chain so the two do not collide.

**Files (every deletion listed):**
- `src/gateway/handlers/mod.rs:374` — `registry.register("plugins.executeCommand", plugins::handle_execute_command);`
- `src/gateway/handlers/mod.rs:1371` — `assert!(registry.has_method("plugins.executeCommand"));`
- `src/gateway/method_census.rs:331` — `("plugins.executeCommand", Class::Admin),`
- `src/gateway/handlers/plugins/handlers/runtime.rs:55-125` — `handle_execute_command` (whole fn + its doc comment) and its `use` of `ExecuteCommandParams`
- `src/gateway/handlers/plugins/handlers/tests.rs:276-330` — `test_handle_execute_command_missing_params`, `_invalid_params`, `_not_found`, and `handle_execute_command` in the `use` at `:3`
- `src/gateway/handlers/plugins/types.rs:173-189` — `ExecuteCommandParams`
- (`src/gateway/handlers/plugins/handlers.rs:25` is `pub use runtime::*;` — nothing to edit there; the glob stops exporting the fn once it is gone)
- `src/extension/plugin_ops.rs:137-158` — `execute_plugin_command`
- `src/extension/loader.rs:392-430` — `PluginLoader::execute_command`; `:585-590` `test_plugin_loader_execute_command_nonexistent`; `:703-706` the `execute_command("mcp-test", …)` assertion inside its test (keep the rest of that test)
- `src/extension/types/skills.rs:44-92` — `DirectCommandResult` + impl (its only readers were the two functions above; `rg DirectCommandResult src` after the cut → 0), and `DirectCommandResult` in `loader.rs:30` / `plugin_ops.rs:6` imports
- `docs/reference/EXTENSION_SYSTEM.md:613-709` — the whole "## Direct Commands (P0.5)" section (from that heading through the `---` before "## Background Services (P1)") is replaced in the SAME commit (R4.4, 判据 §1; text copied from plan-P5P8 Task P5.3 so the two phases write identical bytes whichever lands first — P5.3 Step 1 checks whether this section is already replaced)
- `docs/reference/PLUGIN_SYSTEM.md:481-483` — the namespace-inequality paragraph is edited a SECOND time here (lead addendum to R4.4): P1.10 rewrote it with wording true at P1.10 time, which still names `executeCommand` as surviving; this commit drops it (the wording below is plan-P5P8 P5.3's final text, so P5.3 finds it already written if P4 lands first)

- [ ] **Step 1: Delete, then prove zero references**

Run: `rg -n 'executeCommand|execute_plugin_command|handle_execute_command|ExecuteCommandParams|DirectCommandResult|loader\.execute_command|fn execute_command\b' src interfaces shared qa crates docs/reference --glob '!src/extension/hooks/**'`
Expected: **0 lines** (the only surviving `execute_command` is `HookExecutor::execute_command` in `hooks/executor.rs`, excluded by the glob on purpose).

- [ ] **Step 2: Build and test**

Run: `cargo test -p alephcore --lib gateway::handlers -- --nocapture && cargo test -p alephcore --lib extension::loader -- --nocapture && cargo test -p alephcore --lib gateway::method_census -- --nocapture && cargo test -p aleph-cli -p aleph-tui --no-run`
Expected: PASS; `method_census` must not report a registered-but-unlisted method (the register line and the census row go together).

- [ ] **Step 3: The two doc lines this commit falsifies (判据 §1)**

`docs/reference/PLUGIN_SYSTEM.md:481-483` — P1.10 left it reading (quoted from plan-P1 P1.10 Step 3):

```markdown
⚠️ **两个命名空间的能力集并不相等**：`callTool` / `executeCommand` **只**在复数上，`update` /
`reload` / `config.*` / `marketplace.*` **只**在单数上（`load` / `unload` 于 2026-09-20 CUT——零客户端，
且它们绕过 registry 直接对 WASM loader 寻址，与 mount/unmount 生命周期相悖）。
```

replace with (identical to plan-P5P8 P5.3's final wording):

```markdown
⚠️ **两个命名空间的能力集并不相等**：`callTool` **只**在复数上，`update` / `reload` / `marketplace.*` **只**在
单数上（`executeCommand` / `load` / `unload` 于 2026-09-20 CUT——零客户端，且 `load`/`unload` 绕过 registry
直接对 WASM loader 寻址，与 mount/unmount 生命周期相悖）。
```

(If `config.*` is still registered when this lands — P5 decides the `plugin.config.*` fate — keep `config.*` in the singular list; the sentence must describe the registry as it is at commit time, not as P5 will leave it.)

Then replace the "Direct Commands" section of `docs/reference/EXTENSION_SYSTEM.md:613-709` with exactly this (identical to plan-P5P8 P5.3):

```markdown
## Direct Commands —— ❌ 已删除（2026-09-20）

这里曾有一节（~95 行）描述 `[[commands]] handler = "handlePing"` 式的 manifest、TypeScript
`DirectCommandArgs` / `DirectCommandResult` 签名、以及 `plugins.executeCommand` RPC。

**三者都不再存在。** `CommandRegistration` 在 2026-07-17 折进 `SkillRegistration { skill_type: Command }`
（插件 `commands/*.md` 的 markdown 正文就是那个"handler"），之后 `plugins.executeCommand` 把一段
markdown 当 WASM 导出名去调，零客户端，2026-09-20 连同 `ExtensionManager::execute_plugin_command` 一起 CUT。

插件命令今天的形状：`commands/<name>.md` → 注册为 slash 条目 → 用户 `/name args` 经 `chat.send` →
正文经 `SkillTemplate` 展开（`$ARGUMENTS` / `$N` / `@file` / `` !`cmd` ``）后作为本轮用户内容注入模型
（见 PLUGIN_SYSTEM.md「commands / agents 正文」）。**没有绕过模型的直接命令**——要确定性执行，写一个
工具（WASM 导出或 MCP tool），不要写命令。

> 保留这一节的标题而不是删干净，理由同下方 Channel / Provider 那节。
```

- [ ] **Step 4: Commit**

```bash
git add -A src/gateway/handlers src/gateway/method_census.rs src/extension/plugin_ops.rs src/extension/loader.rs src/extension/types/skills.rs docs/reference/EXTENSION_SYSTEM.md docs/reference/PLUGIN_SYSTEM.md
git commit -m "gateway: CUT plugins.executeCommand (zero clients; markdown body passed as a WASM export)

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

#### Task P4.7e: `disable-model-invocation` on commands — DEVIATION, no code

Aleph has no `SlashCommand` tool: plugin commands are listed for humans only (`commands.list`) and never appear in the model-facing `<available_skills>` block (only `SkillType::Skill` does — `SkillRegistration::is_auto_invocable`, `registry/types.rs:206-209`). `disable-model-invocation: true` therefore changes nothing (already the case) and `false` cannot make a command model-invocable. Recorded as DEVIATION #18 (partial) in §10; P4.7b parses the field so the registration says what the author wrote.

---
### Task P4.8: `agents/*.md` body becomes the sub-agent's system prompt (one mapping for disk and plugin agents)

**Files:**
- Modify: `src/agents/types.rs:212-271` (`AgentDef.system_prompt`), `:272-292` (`new`), new `with_system_prompt`
- Create: `src/agents/system_prompt.rs` (the ONE body→prompt mapping) + `mod system_prompt;` in `src/agents/mod.rs`
- Modify: `src/agents/loader.rs:229-234` (the `let _ = body;` line)
- Modify: `src/extension/mod.rs:242-294` (`plugin_agent_to_def`), `:1469-1509` (test rename to `..._keeps_body`)
- Modify: `src/extension/manifest/parsers.rs:62-71` (`AgentFm` gains `tools`, `permission_mode`, `color`), `:421-440` (`parse_single_agent`)
- Modify: `src/thinker/layers/agent_role.rs:102-122` (`inject`)
- Test: `src/agents/system_prompt.rs`, `src/thinker/layers/agent_role.rs`, `src/extension/mod.rs`, `src/extension/manifest/parsers.rs`, `src/agents/loader.rs`

**Interfaces:**
- Produces:
  ```rust
  // src/agents/types.rs
  /// The markdown body of the agent's definition file, injected verbatim after
  /// the sub-agent role header. `None` for builtins and for files with an
  /// empty body.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub system_prompt: Option<String>,
  impl AgentDef { pub fn with_system_prompt(self, prompt: impl Into<String>) -> Self; }
  // src/agents/system_prompt.rs
  pub const SYSTEM_PROMPT_SOFT_CEILING_CHARS: usize = 10_000;
  pub fn body_to_system_prompt(agent_id: &str, body: &str) -> Option<String>;
  ```

Today: `loader.rs:229-232` — `// Body is intentionally unused … let _ = body;`; `plugin_agent_to_def` (`mod.rs:246-250`) — "the markdown `content` (system-prompt body) is intentionally dropped"; `AgentRoleLayer::inject` (`agent_role.rs:102-122`) pushes the role header and the static `prompt_sections` only. User ruling U5 reverses FL §3.10's "刻意仍不做".

**`permissionMode` → tier: DEVIATION, with the table.** A sub-agent has no tier of its own: it runs on the parent's `ScopedToolService` (`allowlist_tool_service.rs:1-13`, `:118-121` "this wrapper narrows WHICH tools a child may call, never whether a call pauses for a human"), so an `AgentDef` tier field would have zero consumers (R10) and the census in `capability/census.rs` is exactly the kind of guard that would flag it. The value is parsed and logged at `debug!` with the tier it WOULD map to, so the deviation is visible in a log rather than silent:

| CC `permissionMode` | 1:1 Aleph tier | Applied? |
|---|---|---|
| `plan` | `Plan` | no — logged |
| `default` | `Ask` | no — logged |
| `auto` | `Auto` | no — logged |
| `bypassPermissions` | `Full` | no — logged |
| `acceptEdits`, `dontAsk` | none | no — logged as "no Aleph tier" |

`color` is parsed and ignored (UI-only in CC).

- [ ] **Step 1: Write the failing tests**

`src/agents/system_prompt.rs` (new file, tests inline):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_whitespace_bodies_are_none() {
        assert_eq!(body_to_system_prompt("a", ""), None);
        assert_eq!(body_to_system_prompt("a", "  \n\t"), None);
    }

    #[test]
    fn a_body_is_trimmed_and_kept_whole() {
        assert_eq!(
            body_to_system_prompt("a", "\nYou are a reviewer.\n\n## Process\n1. read\n"),
            Some("You are a reviewer.\n\n## Process\n1. read".to_string())
        );
    }

    #[test]
    fn an_oversized_body_is_kept_not_truncated() {
        // CC's 10 000-char ceiling is a soft-fail (warn), not a reject.
        let big = "x".repeat(SYSTEM_PROMPT_SOFT_CEILING_CHARS + 1);
        assert_eq!(body_to_system_prompt("a", &big).map(|s| s.len()), Some(big.len()));
    }
}
```

`src/thinker/layers/agent_role.rs` `mod tests`:

```rust
    #[test]
    fn a_system_prompt_body_is_injected_after_the_role_header() {
        let layer = AgentRoleLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let agent = AgentDef::new("plugin-validator", AgentMode::SubAgent)
            .with_system_prompt("You are an expert plugin validator.");
        let input = LayerInput::basic(&config, &tools).with_agent_def(&agent);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        let header = out.find("Sub-Agent Role").expect("role header");
        let body = out.find("You are an expert plugin validator.").expect("body injected");
        assert!(header < body, "header first, then the author's prompt");
    }

    #[test]
    fn no_system_prompt_keeps_the_previous_bytes() {
        let layer = AgentRoleLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let agent = AgentDef::new("explore", AgentMode::SubAgent);
        let input = LayerInput::basic(&config, &tools).with_agent_def(&agent);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert_eq!(out.matches("---").count(), 1, "exactly the header's separator, nothing appended");
    }
```

`src/extension/mod.rs` — rename `plugin_agent_to_def_maps_subagent_and_drops_body` (`:1469`) to `plugin_agent_to_def_maps_subagent_and_keeps_body`, change the fixture content to `"You are the deployer.\n"` and replace the comment-only expectation with:

```rust
        assert_eq!(def.system_prompt.as_deref(), Some("You are the deployer."));
```

and add:

```rust
    #[test]
    fn disk_and_plugin_agents_share_one_body_mapping() {
        // The same body through both loaders lands as the same prompt —
        // a second mapping is where the two would drift (判据 §16).
        use crate::extension::types::AgentMode as ExtMode;
        let body = "\n\nYou are X.\n\n";
        let reg = crate::extension::AgentRegistration {
            name: "x".into(),
            content: body.into(),
            mode: ExtMode::Subagent,
            ..Default::default()
        };
        let via_plugin = plugin_agent_to_def(&reg).unwrap().system_prompt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.md");
        std::fs::write(&path, format!("---\nid: x\ndescription: d\nwhen_to_use: w\n---{body}")).unwrap();
        let via_disk = crate::agents::loader::parse_file(&path, crate::agents::AgentSource::User)
            .unwrap()
            .system_prompt;
        assert_eq!(via_plugin, via_disk);
        assert_eq!(via_disk.as_deref(), Some("You are X."));
    }
```

`src/extension/manifest/parsers.rs` `mod tests`:

```rust
    #[test]
    fn cc_agent_frontmatter_tools_are_mapped_and_permission_mode_is_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("validator.md"),
            "---\nname: validator\ndescription: Validates plugins\nmodel: inherit\ncolor: yellow\n\
             permissionMode: plan\ntools: [\"Read\", \"Grep\", \"Bash\", \"NotebookEdit\"]\n---\n\
             You are an expert plugin validator.\n",
        )
        .unwrap();
        let caps = parse_agents_dir(dir.path(), "agents", "plug").unwrap();
        let CapabilityDeclaration::Agent(reg) = &caps[0] else { panic!("agent") };
        assert_eq!(reg.content, "You are an expert plugin validator.");
        let tools = reg.tools.as_ref().expect("tools mapped");
        assert_eq!(tools.get("file_read"), Some(&true));
        assert_eq!(tools.get("grep"), Some(&true));
        assert_eq!(tools.get("bash"), Some(&true));
        assert!(!tools.contains_key("NotebookEdit"), "no Aleph counterpart → dropped, not forwarded");
        assert_eq!(reg.model.as_deref(), Some("inherit"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib agents::system_prompt -- --nocapture`
Expected: compile error (module missing); after stubbing, `a_system_prompt_body_is_injected_after_the_role_header` FAILS (`body injected`).

- [ ] **Step 3: Write the implementation**

`src/agents/system_prompt.rs`:

```rust
//! The ONE mapping from an agent definition file's markdown body to
//! `AgentDef::system_prompt`. Disk agents (`loader.rs`) and plugin agents
//! (`extension::plugin_agent_to_def`) both call it; before this module both
//! dropped the body, and a second mapping would be where they drift.

/// Claude Code's stated ceiling for an agent body. Soft: over it we `warn!`
/// and keep the whole text — an author's prompt is not ours to cut.
pub const SYSTEM_PROMPT_SOFT_CEILING_CHARS: usize = 10_000;

/// Trimmed body, or `None` when there is nothing to inject.
#[must_use]
pub fn body_to_system_prompt(agent_id: &str, body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().count() > SYSTEM_PROMPT_SOFT_CEILING_CHARS {
        tracing::warn!(
            agent_id,
            chars = trimmed.chars().count(),
            ceiling = SYSTEM_PROMPT_SOFT_CEILING_CHARS,
            "agent system prompt exceeds Claude Code's soft ceiling; kept whole"
        );
    }
    Some(trimmed.to_string())
}
```

`src/agents/types.rs` — field after `isolation` (doc as in Interfaces), `system_prompt: None` in `new()`, and:

```rust
    /// Set the system prompt body (the agent file's markdown below the frontmatter).
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }
```

`src/agents/loader.rs:229-232` — replace `let _ = body;` with:

```rust
    // The markdown body is the agent's own prompt (Claude Code parity, user
    // ruling 2026-09-20 U5). One mapping shared with plugin agents.
    if let Some(prompt) = crate::agents::system_prompt::body_to_system_prompt(&fm.id, body) {
        def = def.with_system_prompt(prompt);
    }
```

`src/extension/mod.rs:242-294` `plugin_agent_to_def` — replace the doc paragraph "the markdown `content` (system-prompt body) is intentionally dropped …" with "the markdown `content` is the sub-agent's system prompt, through the same mapping the disk loader uses (`agents::system_prompt`)", and before `def.source = …`:

```rust
    if let Some(prompt) = crate::agents::system_prompt::body_to_system_prompt(id, &reg.content) {
        def = def.with_system_prompt(prompt);
    }
```

`src/extension/manifest/parsers.rs:62-71` `AgentFm`:

```rust
#[derive(Debug, Default, Deserialize)]
struct AgentFm {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    model: Option<String>,
    /// CC `tools:` — an array of CC tool names, mapped to Aleph names.
    #[serde(default)]
    tools: Option<Vec<String>>,
    /// CC `permissionMode` — parsed so the deviation is logged, never applied
    /// (a sub-agent runs on its parent's tier; `allowlist_tool_service.rs`).
    #[serde(default, rename = "permissionMode")]
    permission_mode: Option<String>,
    /// CC `color` — UI-only upstream; ignored.
    #[serde(default)]
    color: Option<String>,
}
```

`parse_single_agent` (`:421-440`):

```rust
    let (fm, body): (AgentFm, String) = parse_frontmatter(&content)?;
    let name = fm.name.unwrap_or_else(|| default_name.to_string());
    if let Some(mode) = fm.permission_mode.as_deref() {
        let would_be = match mode {
            "plan" => "Plan", "default" => "Ask", "auto" => "Auto", "bypassPermissions" => "Full",
            _ => "no Aleph tier",
        };
        tracing::debug!(agent = %name, permission_mode = mode, would_be,
            "agent permissionMode is not applied: a sub-agent runs on its parent's tier");
    }
    let tools = fm.tools.map(|names| {
        names
            .iter()
            .filter_map(|n| {
                let mapped = crate::extension::hooks::normalize_cc_tool_entry(n, true);
                if mapped.is_none() {
                    warn!(agent = %name, entry = %n, "agent tools entry has no Aleph tool; dropped");
                }
                mapped
            })
            .map(|t| (t, true))
            .collect::<HashMap<String, bool>>()
    });
    Ok(CapabilityDeclaration::Agent(AgentRegistration {
        name,
        description: fm.description.and_then(|d| if d.is_empty() { None } else { Some(d) }),
        content: body,
        model: fm.model,
        tools,
        color: fm.color,
        plugin_id: plugin_id.to_string(),
        ..Default::default()
    }))
```

`src/thinker/layers/agent_role.rs:102-122` `inject` — after the role header block:

```rust
        // The author's own prompt (the agent file's body), verbatim.
        if let Some(prompt) = agent.system_prompt.as_deref() {
            output.push_str(prompt);
            output.push_str("\n\n---\n\n");
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib agents:: -- --nocapture && cargo test -p alephcore --lib thinker::layers::agent_role -- --nocapture && cargo test -p alephcore --lib extension::tests -- --nocapture && cargo test -p alephcore --lib extension::manifest::parsers -- --nocapture`
Expected: PASS. `AgentRoleLayer` is `LayerStability::Stable`; a per-agent body is byte-stable per agent, so the cached-prompt tests in `prompt_builder/cache.rs` stay green.

**Mutation step:** in `loader.rs` restore `let _ = body;` (drop the mapping call) → `disk_and_plugin_agents_share_one_body_mapping` red (`via_disk == None`). Revert.

- [ ] **Step 5: Commit**

```bash
git add src/agents/system_prompt.rs src/agents/mod.rs src/agents/types.rs src/agents/loader.rs src/extension/mod.rs src/extension/manifest/parsers.rs src/thinker/layers/agent_role.rs
git commit -m "agents: the agent file body is the sub-agent system prompt (disk and plugin, one mapping)

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P4.9: marketplace `source` discriminator — accept both `source.source` and `source.type`

**Files:**
- Modify: `src/extension/marketplace/types.rs:129-138` (`external_kind`)
- Test: `src/extension/marketplace/manifest.rs` `mod tests`

The real file on this machine (`~/.claude/plugins/marketplaces/claude-plugins-official/.claude-plugin/marketplace.json`, read-only, 310 entries: 52 bare-string, 97 `source: "git-subdir"`, 161 `source: "url"`, **0** `type:`) — minimal entries, verbatim:

```json
{"name": "42crunch-api-security-testing", "source": {"source": "git-subdir", "url": "https://github.com/42Crunch-AI/claude-plugins.git", "path": "plugins/api-security-testing", "ref": "v1.5.5", "sha": "30287f5e3f122a646d1ac5ca3ab96e130c52a3ad"}}
{"name": "agentforce-adlc", "source": {"source": "url", "url": "https://github.com/SalesforceAIResearch/agentforce-adlc.git", "sha": "09bf1539d41f9ff355ba3eb5d05d4a75813423bb"}}
{"name": "agent-sdk-dev", "source": "./plugins/agent-sdk-dev"}
```

The live docs spell the discriminator `type` (`{"type": "git-subdir", …}`). `MarketplacePluginSource::External(serde_json::Value)` already parses both (untagged; the value is kept verbatim) — the only reader of the discriminator is `external_kind()`, which reads `source` only. This is a one-function change: **no serde alias is needed** because the object form is not modelled field-by-field (`types.rs:112-116` explains why: five structs with no consumer).

- [ ] **Step 1: Write the failing test**

`src/extension/marketplace/manifest.rs` `mod tests`:

```rust
    /// The shipping marketplace (`claude-plugins-official`, 2026-09-20) spells
    /// the discriminator `source`; the live docs spell it `type`. Both must
    /// answer `external_kind`, or a docs-shaped entry refuses with `'object'`
    /// instead of naming its kind.
    #[test]
    fn both_discriminator_spellings_name_the_source_kind() {
        let manifest = parse_marketplace_json_content(
            r#"{
              "name": "official-shaped",
              "plugins": [
                {"name": "disk-form", "source": {"source": "git-subdir", "url": "https://github.com/x/y.git", "path": "plugins/z", "ref": "v1", "sha": "30287f5e3f122a646d1ac5ca3ab96e130c52a3ad"}},
                {"name": "docs-form", "source": {"type": "git-subdir", "url": "https://github.com/x/y.git", "path": "plugins/z"}},
                {"name": "url-form", "source": {"source": "url", "url": "https://github.com/x/y.git", "sha": "09bf1539d41f9ff355ba3eb5d05d4a75813423bb"}},
                {"name": "npm-docs", "source": {"type": "npm", "package": "@scope/name", "version": "^1"}},
                {"name": "path-form", "source": "./plugins/agent-sdk-dev"}
              ]
            }"#,
        )
        .unwrap();
        let kinds: Vec<Option<&str>> = manifest.plugins.iter().map(|p| p.source.external_kind()).collect();
        assert_eq!(kinds, vec![Some("git-subdir"), Some("git-subdir"), Some("url"), Some("npm"), None]);
        assert_eq!(manifest.plugins[4].source.as_relative_path(), Some("./plugins/agent-sdk-dev"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib extension::marketplace::manifest::tests::both_discriminator -- --nocapture`
Expected: FAIL — `[Some("git-subdir"), None, Some("url"), None, None]` (the `type:` entries answer `None`).

- [ ] **Step 3: Write the implementation**

`types.rs:129-138`:

```rust
    /// The discriminator of an object form (`"github"`, `"npm"`, `"git-subdir"`, …),
    /// for error messages. `None` for the path form.
    ///
    /// Two spellings, both real: the shipping `claude-plugins-official`
    /// marketplace (2026-09-20) writes `{"source": "git-subdir", …}` — the
    /// key name reused one level down — while the live docs write
    /// `{"type": "git-subdir", …}`. Disk first, since that is what is
    /// installed today; a document carrying both agrees with itself or the
    /// on-disk spelling wins.
    #[must_use]
    pub fn external_kind(&self) -> Option<&str> {
        match self {
            Self::Path(_) => None,
            Self::External(v) => v
                .get("source")
                .or_else(|| v.get("type"))
                .and_then(serde_json::Value::as_str),
        }
    }
```

Also update the doc on the enum (`types.rs:92-108`): "an object tagged by a `source` discriminator" → "an object tagged by a `source` (on-disk) or `type` (docs) discriminator".

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib extension::marketplace -- --nocapture`
Expected: PASS, including `an_object_source_does_not_take_the_whole_marketplace_down`.

- [ ] **Step 5: Same-commit doc sentence (R4.6, 判据 §1)**

`docs/reference/PLUGIN_SYSTEM.md:499` "### source 分类只有一个答案（2026-08-20）" — append one paragraph at the end of that subsection (after the "顺带收敛掉三族重复" paragraph, before the next `##`):

```markdown
**marketplace 条目的 `source` 对象也只有一个读者（2026-09-20）**：判别键在磁盘上拼作 `source.source`
（`claude-plugins-official` 当天的 310 条里 258 条对象形全是这个拼法，0 条 `type`），在官方文档里拼作
`source.type`。两种拼法都进 `MarketplacePluginSource::external_kind()`（`marketplace/types.rs`）——先读
`source` 再读 `type`——而不是各建一个 serde 字段：对象形本来就不逐字段建模（五个无消费者的 struct，R10），
拒绝消息引用的那个词才是唯一要读的东西。
```

- [ ] **Step 6: Commit**

```bash
git add src/extension/marketplace/types.rs src/extension/marketplace/manifest.rs docs/reference/PLUGIN_SYSTEM.md
git commit -m "marketplace: read the source discriminator from source.source or source.type

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P4.10: Read-only discovery of `~/.claude/plugins/` (`PluginOrigin::ClaudeCache`, default disabled)

**Files:**
- Create: `src/discovery/claude_cache.rs`
- Modify: `src/discovery/types.rs` — after P2.3: `GlobalRoot::ClaudeCache`, `DiscoveryScope::source()`'s new arm, `DiscoverySource::ClaudeCache` (the derived label `PluginOrigin::classify` reads); `src/discovery/mod.rs` (`mod claude_cache;`, `DiscoveryConfig.claude_home_override`), `src/discovery/scanner.rs:214-240` (`discover_plugins_with_extra`)
- Modify: `src/extension/types/plugins.rs:51-119` (`PluginOrigin::ClaudeCache`, `classify`, `priority`, new `label`, `enabled_by_default`), `:14-43` (`PluginInfo.origin`)
- Modify: `src/extension/plugin_trust.rs:108-116` (`allows` arm)
- Modify: `src/extension/plugin_state.rs:175-182` (`is_enabled` → `is_enabled_for`)
- Modify: `src/extension/lifecycle.rs` — **P1.9's** `admit` (owner trust + `plugins.toml` `is_enabled`) and the `migrate_legacy_disabled_marker` call in P1.9's `load_all` (R4.3; at 3ddc1f2e7 these were `src/extension/mod.rs:630-676`, deleted by P1.9/P1.11). `build_record` (origin + kind onto the row) and `write_failed_row` are untouched — the origin they stamp is what `admit` reads.
- Modify: `src/extension/visibility.rs` — **P2.3's** `ScopeKey::from_discovery` (wildcard-free over `GlobalRoot`; the new variant does not compile until its arm exists) + P2.3's test loop (R4.3)
- Modify: `src/extension/plugin_ops.rs:293-317` (`PluginInfo { origin, … }`)
- Modify: `shared/protocol/src/plugins.rs:71-135` (`PluginRow.origin`), `src/gateway/handlers/plugins/types.rs:34-55` (`plugin_row`), `src/gateway/handlers/plugins/handlers/install.rs:104-124` (literal)
- Test: `src/discovery/claude_cache.rs`, `src/extension/plugin_state.rs`, `src/extension/plugin_trust.rs`, `src/extension/mod.rs` (`mod tests`, using the isolated-manager helper at `:1516+`)

**Interfaces:**
- Produces:
  ```rust
  // src/discovery/types.rs (P2.3's final shape, one variant wider)
  pub enum GlobalRoot { Aleph, Claude, /// `~/.claude/plugins/cache/…` — Claude Code's own installed-plugin cache, read-only.
                        ClaudeCache }
  // DiscoveryScope::source(): Global(GlobalRoot::ClaudeCache) => DiscoverySource::ClaudeCache
  pub enum DiscoverySource { AlephGlobal, ClaudeGlobal, Project, ClaudeCache }
  // src/discovery/claude_cache.rs
  pub(crate) const INSTALLED_PLUGINS_FILE: &str = "installed_plugins.json";
  pub(crate) const CLAUDE_CACHE_PRIORITY: u32 = 5;
  pub(crate) fn discover_claude_cache(claude_home: &Path) -> Vec<DiscoveredPath>;
  pub(crate) fn parse_installed_plugins(json: &str) -> Result<Vec<CachedPlugin>, String>;
  pub(crate) struct CachedPlugin { pub key: String /* name@marketplace */, pub marketplace: String, pub name: String, pub version: String }
  impl CachedPlugin { pub fn cache_dir(&self, claude_home: &Path) -> PathBuf } // plugins/cache/<marketplace>/<name>/<version>
  // src/extension/types/plugins.rs
  pub enum PluginOrigin { Config, Workspace, Global, Bundled, ClaudeCache }
  impl PluginOrigin { pub const fn label(self) -> &'static str; pub const fn enabled_by_default(self) -> bool; }
  // src/extension/plugin_state.rs
  pub fn is_enabled_for(&self, plugin_id: &str, origin: PluginOrigin) -> bool;   // absent ⇒ origin.enabled_by_default()
  // shared/protocol/src/plugins.rs — PluginRow
  #[serde(default)] pub origin: String,   // "config" | "workspace" | "global" | "bundled" | "claude_cache" (PluginOrigin::label)
  ```
  Scope key: `ClaudeCache` rows are `ScopeKey::Global` — the `GlobalRoot::ClaudeCache` arm in P2.3's `visibility.rs::from_discovery` is Step 3 of this task (R4.3); P2.3 left that match wildcard-free precisely so this task cannot forget it. No `ScanDirectory` is added for `~/.claude/plugins/`: `ScanDirectory` feeds `discover_component` (skills / commands / agents under a root), and the cache is not a component root — it is an index (`installed_plugins.json`) resolved to plugin dirs, which is why it enters through `discover_plugins_with_extra` as `DiscoveredPath::global(dir, GlobalRoot::ClaudeCache, …)` (scope `DiscoveryScope::Global(GlobalRoot::ClaudeCache)`, the same scope value a `ScanDirectory` would have carried).

**The real file (read-only, this machine, 2026-09-20):** `~/.claude/plugins/installed_plugins.json` is `{"version": 2, "plugins": {"<name>@<marketplace>": [ {scope, installPath, version, installedAt, lastUpdated, gitCommitSha?} ]}}` — 15 keys, every array has exactly 1 element, every `scope` is `"user"`; the extra-key set observed is exactly `gitCommitSha installPath installedAt lastUpdated scope version`. Minimal entry:

```json
{"version": 2, "plugins": {"clangd-lsp@claude-plugins-official": [{"scope": "user", "installPath": "~/.claude/plugins/cache/claude-plugins-official/clangd-lsp/1.0.0", "version": "1.0.0", "installedAt": "2025-12-23T09:04:40.454Z", "lastUpdated": "2025-12-23T09:04:40.454Z"}]}}
```

Resolution rule (spec §3.6 #10): `cache/<marketplace>/<plugin>/<version>/` DERIVED from the key and `version` — `installPath` is an absolute path on someone's machine and is not followed (a symlinked or relocated path would otherwise be enumerated as if it lived in the cache; `scan_plugin_parent`'s `is_existing_dir_no_follow` rule, same reason). A derived dir that does not exist or has no manifest (the three LSP-only plugins ship no `.claude-plugin/plugin.json`) is skipped at `debug!`. A malformed file, or `version != 2`, skips the whole source with ONE `warn!` and discovers nothing from it (P7: a foreign file's shape is not ours to trust).

**Never writing under `~/.claude/`** — what could: the legacy `.disabled` marker migration (P1.9 `lifecycle.rs::migrate_legacy_disabled_marker`, `remove_file`; at 3ddc1f2e7 `mod.rs:630-654`) — gated off for this origin; `plugin update` / `uninstall` derive their paths from `default_plugins_dir()` (`manage.rs:197-198`), never from `root_dir`, so a ClaudeCache id answers "Plugin not found" there (documented: remove it in Claude Code); `CLAUDE_PLUGIN_DATA` lives under `~/.aleph` (`plugin_vars.rs:52`). The guard test below snapshots the fixture tree before and after `load_all`.

- [ ] **Step 1: Write the failing tests**

`src/discovery/claude_cache.rs` `mod tests`:

```rust
    const REAL_SHAPE: &str = r#"{"version": 2, "plugins": {
      "clangd-lsp@claude-plugins-official": [{"scope": "user", "installPath": "/Users/someone/.claude/plugins/cache/claude-plugins-official/clangd-lsp/1.0.0", "version": "1.0.0", "installedAt": "2025-12-23T09:04:40.454Z", "lastUpdated": "2025-12-23T09:04:40.454Z"}],
      "superpowers@claude-plugins-official": [{"scope": "user", "installPath": "/Users/someone/.claude/plugins/cache/claude-plugins-official/superpowers/6.3.0", "version": "6.3.0", "installedAt": "2026-01-01T00:00:00.000Z", "lastUpdated": "2026-09-20T00:00:00.000Z", "gitCommitSha": "0123456789abcdef0123456789abcdef01234567"}],
      "plugin-dev@claude-plugins-official": [{"scope": "user", "installPath": "/x", "version": "c447c3207a42", "installedAt": "2026-09-20T00:00:00.000Z", "lastUpdated": "2026-09-20T00:00:00.000Z"}]
    }}"#;

    #[test]
    fn parses_the_real_shape_and_derives_the_cache_dir_from_key_and_version() {
        let cached = parse_installed_plugins(REAL_SHAPE).unwrap();
        assert_eq!(cached.len(), 3);
        let sp = cached.iter().find(|c| c.name == "superpowers").unwrap();
        assert_eq!(sp.marketplace, "claude-plugins-official");
        assert_eq!(sp.version, "6.3.0");
        assert_eq!(
            sp.cache_dir(Path::new("/home/u/.claude")),
            PathBuf::from("/home/u/.claude/plugins/cache/claude-plugins-official/superpowers/6.3.0")
        );
        // A git-sha pseudo-version is an opaque directory name, not semver.
        let pd = cached.iter().find(|c| c.name == "plugin-dev").unwrap();
        assert!(pd.cache_dir(Path::new("/h")).ends_with("plugin-dev/c447c3207a42"));
        // `installPath` is NOT what is followed.
        assert!(!format!("{:?}", cached).contains("/Users/someone"), "installPath must not leak into the derived path");
    }

    #[test]
    fn a_malformed_or_foreign_version_file_discovers_nothing() {
        assert!(parse_installed_plugins("{not json").is_err());
        assert!(parse_installed_plugins(r#"{"version": 3, "plugins": {}}"#).is_err(), "an unknown schema version is not ours to guess");
        assert!(parse_installed_plugins(r#"{"version": 2, "plugins": {"bad-key-no-at": [{"version": "1"}]}}"#).unwrap().is_empty());
    }

    #[test]
    fn discovery_lists_only_cache_dirs_that_carry_a_manifest() {
        let home = tempfile::tempdir().unwrap();
        let plugins = home.path().join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        std::fs::write(plugins.join(INSTALLED_PLUGINS_FILE), REAL_SHAPE).unwrap();
        // superpowers: a real manifest. clangd-lsp: dir exists, no manifest (LSP-only). plugin-dev: dir absent.
        let sp = plugins.join("cache/claude-plugins-official/superpowers/6.3.0/.claude-plugin");
        std::fs::create_dir_all(&sp).unwrap();
        std::fs::write(sp.join("plugin.json"), r#"{"name":"superpowers"}"#).unwrap();
        std::fs::create_dir_all(plugins.join("cache/claude-plugins-official/clangd-lsp/1.0.0")).unwrap();
        let found = discover_claude_cache(home.path());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].scope, DiscoveryScope::Global(GlobalRoot::ClaudeCache));
        assert_eq!(found[0].source(), DiscoverySource::ClaudeCache, "the derived label classify() reads");
        assert_eq!(found[0].priority, CLAUDE_CACHE_PRIORITY);
        assert!(found[0].path.ends_with("superpowers/6.3.0"));
    }

    #[test]
    fn a_missing_file_is_a_silent_no_op() {
        let home = tempfile::tempdir().unwrap();
        assert!(discover_claude_cache(home.path()).is_empty());
    }
```

`src/extension/plugin_state.rs` `mod tests`:

```rust
    #[test]
    fn absent_means_disabled_only_for_the_claude_cache_origin() {
        let cfg = PluginsConfig::default();
        assert!(cfg.is_enabled_for("x", PluginOrigin::Global));
        assert!(cfg.is_enabled_for("x", PluginOrigin::Workspace));
        assert!(!cfg.is_enabled_for("x", PluginOrigin::ClaudeCache), "a Claude Code install is not an Aleph opt-in");
        let mut cfg = PluginsConfig::default();
        assert!(cfg.set_enabled("x", true));
        assert!(cfg.is_enabled_for("x", PluginOrigin::ClaudeCache), "one explicit verb turns it on");
        assert!(cfg.set_enabled("x", false));
        assert!(!cfg.is_enabled_for("x", PluginOrigin::Global), "explicit false still wins everywhere");
    }
```

`src/extension/plugin_trust.rs` `mod tests` (next to `:145-170`):

```rust
    #[test]
    fn claude_cache_is_an_untrusted_origin_under_enforcement() {
        let policy = OwnerTrustPolicy::restrictive(["vouched".to_string()]);
        assert!(policy.allows("vouched", PluginOrigin::ClaudeCache));
        assert!(!policy.allows("unknown", PluginOrigin::ClaudeCache), "code installed by another tool is not exempt");
        assert!(OwnerTrustPolicy::permissive().allows("unknown", PluginOrigin::ClaudeCache));
    }
```

`src/extension/mod.rs` `mod tests` (the isolated-manager helper at `:1516+` builds a manager with `extra_plugin_parents` and a temp `plugins.toml`; extend it with a `claude_home` override — see Step 3):

```rust
    #[tokio::test]
    async fn a_claude_cache_plugin_loads_disabled_and_never_writes_under_claude_home() {
        let claude_home = tempfile::tempdir().unwrap();
        let plugins = claude_home.path().join("plugins");
        let root = plugins.join("cache/qa-market/qa-cc/1.0.0");
        std::fs::create_dir_all(root.join(".claude-plugin")).unwrap();
        std::fs::create_dir_all(root.join("commands")).unwrap();
        std::fs::write(root.join(".claude-plugin/plugin.json"), r#"{"name":"qa-cc","version":"1.0.0"}"#).unwrap();
        std::fs::write(root.join("commands/hello.md"), "---\ndescription: hi\n---\nSay hi to $ARGUMENTS.\n").unwrap();
        std::fs::write(
            plugins.join("installed_plugins.json"),
            r#"{"version":2,"plugins":{"qa-cc@qa-market":[{"scope":"user","installPath":"/elsewhere","version":"1.0.0","installedAt":"2026-01-01T00:00:00Z","lastUpdated":"2026-01-01T00:00:00Z"}]}}"#,
        ).unwrap();
        // Also plant a stray legacy marker: the migration must NOT remove it here.
        std::fs::write(root.join(".disabled"), "").unwrap();
        let snapshot = |dir: &std::path::Path| -> Vec<(PathBuf, std::time::SystemTime)> {
            let mut out = Vec::new();
            fn walk(d: &std::path::Path, out: &mut Vec<(PathBuf, std::time::SystemTime)>) {
                for e in std::fs::read_dir(d).unwrap().flatten() {
                    let p = e.path();
                    out.push((p.clone(), e.metadata().unwrap().modified().unwrap()));
                    if p.is_dir() { walk(&p, out); }
                }
            }
            walk(dir, &mut out);
            out.sort();
            out
        };
        let before = snapshot(claude_home.path());

        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let scratch = tempfile::tempdir().unwrap();
        let (manager, _cfg_path) = isolated_manager_with_claude_home(scratch.path(), claude_home.path()).await;
        // P1.9's `load_all` (discover → admit → mount every admitted plugin).
        manager.load_all().await.unwrap();
        let info = manager.get_plugin_info().await;
        let row = info.iter().find(|p| p.name == "qa-cc").expect("discovered");
        assert_eq!(row.origin, "claude_cache");
        assert_eq!(row.status, "disabled", "a CC-installed plugin is off until Aleph is told otherwise");
        assert!(!row.enabled);

        assert!(manager.set_plugin_enabled("qa-cc", true).await);
        let info = manager.get_plugin_info().await;
        assert!(info.iter().find(|p| p.name == "qa-cc").unwrap().enabled);

        assert_eq!(snapshot(claude_home.path()), before, "nothing under ~/.claude may change");
        assert!(root.join(".disabled").exists(), "the legacy-marker migration must not touch a foreign tree");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib discovery::claude_cache -- --nocapture`
Expected: compile errors (module, variants missing).

- [ ] **Step 3: Write the implementation**

`src/discovery/types.rs` — P2.3's final shape, one variant wider in each of the three places the compiler will name:

```rust
pub enum GlobalRoot {
    /// `~/.aleph` (`DiscoverySource::AlephGlobal`).
    Aleph,
    /// `~/.claude` (`DiscoverySource::ClaudeGlobal`).
    Claude,
    /// `~/.claude/plugins/cache/<marketplace>/<plugin>/<version>` — Claude
    /// Code's own installed-plugin cache, read-only, discovered from
    /// `installed_plugins.json` (`DiscoverySource::ClaudeCache`).
    ClaudeCache,
}
```

`DiscoveryScope::source()` gains `Self::Global(GlobalRoot::ClaudeCache) => DiscoverySource::ClaudeCache,` and `DiscoverySource` gains the `ClaudeCache` variant it names (doc: "the derived label for `GlobalRoot::ClaudeCache`; consumed by `PluginOrigin::classify`").

`src/discovery/claude_cache.rs`:

```rust
//! Read-only discovery of the plugins Claude Code has installed.
//!
//! `~/.claude/plugins/installed_plugins.json` (schema `version: 2`) names each
//! install as `<name>@<marketplace>` with a `version`; the plugin tree lives
//! at `plugins/cache/<marketplace>/<name>/<version>/`. That path is DERIVED
//! from the key — the file's `installPath` is an absolute path on whatever
//! machine wrote it and is never followed. Aleph reads this tree and never
//! writes under it (user ruling U6); the enable bit lives in Aleph's own
//! `plugins.toml`, default off (`PluginOrigin::enabled_by_default`).

use std::path::{Path, PathBuf};

use super::scanner::{has_plugin_manifest, is_existing_dir_no_follow};
use super::types::{DiscoveredPath, GlobalRoot};
use tracing::{debug, warn};

pub(crate) const INSTALLED_PLUGINS_FILE: &str = "installed_plugins.json";
/// Below `AlephGlobal` (10): on a same-id contest an Aleph install wins
/// (`load_all` walks highest priority first and the first registration
/// keeps the id).
pub(crate) const CLAUDE_CACHE_PRIORITY: u32 = 5;
const SUPPORTED_SCHEMA_VERSION: u64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CachedPlugin {
    pub key: String,
    pub marketplace: String,
    pub name: String,
    pub version: String,
}

impl CachedPlugin {
    pub(crate) fn cache_dir(&self, claude_home: &Path) -> PathBuf {
        claude_home
            .join("plugins")
            .join("cache")
            .join(&self.marketplace)
            .join(&self.name)
            .join(&self.version)
    }
}

#[derive(serde::Deserialize)]
struct InstalledPlugins {
    version: u64,
    #[serde(default)]
    plugins: std::collections::BTreeMap<String, Vec<InstalledEntry>>,
}

#[derive(serde::Deserialize)]
struct InstalledEntry {
    version: String,
}

/// Parse the file's text. `Err` = not a shape this reader understands (the
/// caller skips the whole source with one warn). Entries whose key is not
/// `<name>@<marketplace>` are skipped individually.
pub(crate) fn parse_installed_plugins(json: &str) -> Result<Vec<CachedPlugin>, String> {
    let doc: InstalledPlugins = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if doc.version != SUPPORTED_SCHEMA_VERSION {
        return Err(format!(
            "installed_plugins.json schema version {} (this reader knows {SUPPORTED_SCHEMA_VERSION})",
            doc.version
        ));
    }
    let mut out = Vec::new();
    for (key, entries) in doc.plugins {
        let Some((name, marketplace)) = key.split_once('@') else {
            debug!(key, "installed_plugins.json key is not <name>@<marketplace>; skipped");
            continue;
        };
        if name.is_empty() || marketplace.is_empty() {
            continue;
        }
        for entry in entries {
            if entry.version.is_empty() || entry.version.contains('/') || entry.version.contains("..") {
                continue;
            }
            out.push(CachedPlugin {
                key: key.clone(),
                marketplace: marketplace.to_string(),
                name: name.to_string(),
                version: entry.version,
            });
        }
    }
    Ok(out)
}

/// Every cache dir named by the file that exists and carries a plugin
/// manifest. Missing file → nothing; unreadable/foreign file → one `warn!`
/// and nothing.
pub(crate) fn discover_claude_cache(claude_home: &Path) -> Vec<DiscoveredPath> {
    let file = claude_home.join("plugins").join(INSTALLED_PLUGINS_FILE);
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            warn!(path = %file.display(), error = %e, "cannot read Claude Code's installed_plugins.json; source skipped");
            return Vec::new();
        }
    };
    let cached = match parse_installed_plugins(&text) {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %file.display(), error = %e, "installed_plugins.json is not a shape this build reads; source skipped");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for plugin in cached {
        let dir = plugin.cache_dir(claude_home);
        if !is_existing_dir_no_follow(&dir) {
            debug!(key = %plugin.key, dir = %dir.display(), "cache dir absent; skipped");
            continue;
        }
        if !has_plugin_manifest(&dir) {
            debug!(key = %plugin.key, "no plugin manifest (marketplace-inline plugin, e.g. LSP-only); skipped");
            continue;
        }
        // Scope = a global root of its own (`DiscoveryScope::Global(GlobalRoot::ClaudeCache)`):
        // per-user, no project, and distinguishable from `~/.claude` proper.
        out.push(DiscoveredPath::global(dir, GlobalRoot::ClaudeCache, CLAUDE_CACHE_PRIORITY));
    }
    out
}
```

(`is_existing_dir_no_follow` (`scanner.rs:396`) and `has_plugin_manifest` (`scanner.rs:430`) are private free functions today — make them `pub(crate)` in `scanner.rs` and import them as `use super::scanner::{has_plugin_manifest, is_existing_dir_no_follow};`; do not copy them.)

`src/discovery/scanner.rs:214-240` `discover_plugins_with_extra` — after the `extra_parents` loop:

```rust
        // Claude Code's own installs, read-only. Gated by the same knob as
        // `~/.claude/{skills,commands,agents}` (`scan_claude_dirs`).
        if let Some(claude_home) = self.claude_home.as_deref() {
            discovered.extend(super::claude_cache::discover_claude_cache(claude_home));
        }
```

`src/extension/types/plugins.rs`:

```rust
pub enum PluginOrigin {
    Config,
    Workspace,
    Global,
    Bundled,
    /// Installed by Claude Code under `~/.claude/plugins/cache/…`; read-only,
    /// disabled until `plugins.toml` says otherwise.
    ClaudeCache,
}
impl PluginOrigin {
    pub const fn priority(&self) -> u8 { match self { Self::Config => 4, Self::Workspace => 3, Self::Global => 2, Self::Bundled => 1, Self::ClaudeCache => 1 } }
    /// Stable lowercase wire label (`PluginRow.origin`).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self { Self::Config => "config", Self::Workspace => "workspace", Self::Global => "global", Self::Bundled => "bundled", Self::ClaudeCache => "claude_cache" }
    }
    /// Whether a plugin with NO `plugins.toml` entry loads. Everything Aleph
    /// installed is an opt-in already; a Claude Code install is not.
    #[must_use]
    pub const fn enabled_by_default(self) -> bool { !matches!(self, Self::ClaudeCache) }
    pub const fn classify(source: crate::discovery::DiscoverySource) -> Self {
        match source {
            crate::discovery::DiscoverySource::Project => Self::Workspace,
            crate::discovery::DiscoverySource::AlephGlobal | crate::discovery::DiscoverySource::ClaudeGlobal => Self::Global,
            crate::discovery::DiscoverySource::ClaudeCache => Self::ClaudeCache,
        }
    }
}
```

(`PluginOrigin` needs `Copy` for `label(self)`; it is `#[derive(Debug, Clone, Copy, PartialEq, Eq, …)]` today — verify at `:49`.) `PluginInfo` gains `#[serde(default)] pub origin: String`.

`src/extension/plugin_trust.rs:112-113`: `PluginOrigin::Workspace | PluginOrigin::Global | PluginOrigin::ClaudeCache => self.allowlist.contains(plugin_id),` (+ the refusal-message tests that enumerate origins).

`src/extension/plugin_state.rs:175-182`:

```rust
    /// Whether `plugin_id` should load. An id with no recorded preference
    /// falls back to its ORIGIN's default (`PluginOrigin::enabled_by_default`):
    /// everything Aleph installed loads; a Claude Code install stays off until
    /// one `plugin_manage enable`. Explicit `true`/`false` wins everywhere.
    #[must_use]
    pub fn is_enabled_for(&self, plugin_id: &str, origin: crate::extension::PluginOrigin) -> bool {
        self.entries
            .get(plugin_id)
            .and_then(|e| e.enabled)
            .unwrap_or(origin.enabled_by_default())
    }

    /// `is_enabled_for` with the pre-existing "absent ⇒ enabled" default — for
    /// callers that have no origin in hand (`plugin_manage`, tests).
    #[must_use]
    pub fn is_enabled(&self, plugin_id: &str) -> bool {
        self.is_enabled_for(plugin_id, crate::extension::PluginOrigin::Global)
    }
```

`src/extension/lifecycle.rs` (P1.9 — the shape P1 leaves behind, quoted from plan-P1 P1.9; at 3ddc1f2e7 the same two facts lived at `mod.rs:630-662`):

P1.9's `admit`:

```rust
    async fn admit(&self, id: &str, origin: PluginOrigin) -> Result<(), MountError> {
        let trust_allows = self
            .owner_trust_policy
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .allows(id, origin);
        if !trust_allows {
            return Err(MountError::Blocked { id: id.to_string(), origin: format!("{origin:?}") });
        }
        if !self.plugins_config.read().await.is_enabled(id) {
            return Err(MountError::Disabled(id.to_string()));
        }
        Ok(())
    }
```

change the preference line to the origin-aware predicate (it already has `origin` in hand):

```rust
        if !self.plugins_config.read().await.is_enabled_for(id, origin) {
            return Err(MountError::Disabled(id.to_string()));
        }
```

P1.9's `load_all` call `self.migrate_legacy_disabled_marker(dir_path, &plugin_id).await;` (inside the per-directory loop, where `found.origin` is in scope) becomes:

```rust
            // A foreign tree (Claude Code's cache) is never written: no
            // marker migration there, whatever it contains.
            if found.origin != PluginOrigin::ClaudeCache {
                self.migrate_legacy_disabled_marker(dir_path, &plugin_id).await;
            }
```

`src/extension/visibility.rs` — P2.3's final `ScopeKey::from_discovery` (quoted from plan-P2P3 P2.3; wildcard-free by design, so adding `GlobalRoot::ClaudeCache` is a compile error `non-exhaustive patterns: … Global(ClaudeCache) not covered` until this arm exists):

```rust
    #[must_use]
    pub fn from_discovery(d: &crate::discovery::DiscoveredPath) -> Self {
        use crate::discovery::{DiscoveryScope, GlobalRoot};
        match &d.scope {
            DiscoveryScope::Project { root } => Self::project(root),
            DiscoveryScope::Global(GlobalRoot::Aleph | GlobalRoot::Claude) => Self::Global,
        }
    }
```

becomes (the cache is a per-user tree with no project — joined into the `Global` pattern by name, not by `_`):

```rust
        match &d.scope {
            DiscoveryScope::Project { root } => Self::project(root),
            DiscoveryScope::Global(GlobalRoot::Aleph | GlobalRoot::Claude | GlobalRoot::ClaudeCache) => {
                Self::Global
            }
        }
```

and P2.3's test loop `for global in [GlobalRoot::Aleph, GlobalRoot::Claude]` (in `from_discovery_yields_project_only_for_project_scopes`) gains `GlobalRoot::ClaudeCache`.

**Every exhaustive match this round's two new variants touch** (`rg -n 'GlobalRoot::|DiscoverySource::|PluginOrigin::' src --glob '!*/tests*'` after P1/P2 — the compiler names each one): `discovery/types.rs` `DiscoveryScope::source` (`GlobalRoot`), `visibility.rs::from_discovery` (`GlobalRoot`), `types/plugins.rs` `classify` (`DiscoverySource`) and `priority` / `label` / `enabled_by_default` (`PluginOrigin`), `plugin_trust.rs:112-113 allows` (`PluginOrigin`), `plugin_ops.rs:300 PluginInfo` (label string, no match). The Panel/TUI/CLI never match on the enum (they read `PluginRow.origin` as a string).

`src/extension/plugin_ops.rs:300`: `origin: record.origin.label().to_string(),`. `shared/protocol/src/plugins.rs` `PluginRow`: `#[serde(default)] pub origin: String` (doc: "`PluginOrigin::label`; empty from a server that predates the field"); `handlers/plugins/types.rs:35` `origin: info.origin,`; `install.rs:104` `origin: "global".into(),`.

Test helper: `DiscoveryConfig` (`discovery/mod.rs:32-44`: `working_dir`, `scan_claude_dirs`, `scan_project_dirs`, `max_upward_depth`) gains `pub claude_home_override: Option<PathBuf>` (default `None`; `DirectoryScanner::new` at `scanner.rs:35-49` uses it in place of `claude_home_dir()` when set — production never sets it, tests do; same reason the manager carries the test-only `extra_plugin_parents`, `mod.rs:214-216`). The `extension/mod.rs` test helper `isolated_manager` (`:1521-1536`) gets a sibling:

```rust
    async fn isolated_manager_with_claude_home(dir: &std::path::Path, claude_home: &std::path::Path) -> (ExtensionManager, PathBuf) {
        let cfg_path = dir.join("plugins.toml");
        let manager = ExtensionManager::new(ExtensionConfig {
            discovery: DiscoveryConfig {
                working_dir: dir.to_path_buf(),
                scan_claude_dirs: true,
                scan_project_dirs: false,
                max_upward_depth: 0,
                claude_home_override: Some(claude_home.to_path_buf()),
            },
            plugins_config_path: Some(cfg_path.clone()),
            extra_plugin_parents: vec![],
        })
        .await
        .unwrap();
        (manager, cfg_path)
    }
```

(the test above calls it as `isolated_manager_with_claude_home(scratch.path(), claude_home.path())` with a second `tempdir` for `scratch`; `IsolatedAlephHome::new()` first, as `a_disabled_plugin_stays_inactive_across_a_fresh_load` does at `:1560`). Every other `DiscoveryConfig { … }` literal (`:1524`, and `rg -n 'DiscoveryConfig \{' src`) gains `claude_home_override: None`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib discovery -- --nocapture && cargo test -p alephcore --lib extension::plugin_state -- --nocapture && cargo test -p alephcore --lib extension::plugin_trust -- --nocapture && cargo test -p alephcore --lib extension::tests -- --nocapture && cargo test -p aleph-panel --lib --no-run && cargo test -p aleph-cli -p aleph-tui --no-run`
Expected: PASS. `plugin_manage list` now serialises `origin` (it serialises `PluginInfo` verbatim, `plugin_manage.rs:192-198`).

**Mutation steps:** (1) make `enabled_by_default` return `true` for `ClaudeCache` → `absent_means_disabled_only_for_the_claude_cache_origin` and the manager test (`status == "disabled"`) red. (2) Remove the `!= ClaudeCache` guard on the marker migration in P1.9's `load_all` → the manager test red on `.disabled` / snapshot equality. (3) Remove `| GlobalRoot::ClaudeCache` from the `from_discovery` pattern → **red at compile time** (`non-exhaustive patterns`), which is the recorded red for this arm; then, with the arm present, make it `Self::project(&d.path)` → P2.3's loop test red on `ClaudeCache`. Revert all three.

- [ ] **Step 5: Commit**

```bash
git add src/discovery/claude_cache.rs src/discovery/mod.rs src/discovery/types.rs src/discovery/scanner.rs src/extension/types/plugins.rs src/extension/plugin_trust.rs src/extension/plugin_state.rs src/extension/lifecycle.rs src/extension/visibility.rs src/extension/mod.rs src/extension/plugin_ops.rs shared/protocol/src/plugins.rs src/gateway/handlers/plugins/types.rs src/gateway/handlers/plugins/handlers/install.rs
git commit -m "discovery: read-only ClaudeCache origin for ~/.claude/plugins (default disabled)

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P4.11: `allowed-tools` — command restricts, skill pre-grants

**Files:**
- Modify: `src/gateway/execution_engine/slash_skill_scope.rs` (new key + encoder/decoder for the pre-grant set)
- Modify: `src/gateway/execution_engine/execute.rs:776-782` (split by registration kind)
- Modify: `src/gateway/execution_engine/turn_permissions.rs:305-320` (`resolve_turn_permissions`: fold the pre-grant set into `merged`)
- Test: `slash_skill_scope.rs` `mod tests`, `turn_permissions.rs` `mod tests`

**What the skill path does today (verified):** `skill/manifest.rs:107-147` parses `allowed-tools` (both YAML forms, via `normalize_allowed_tools`); it reaches `SkillInfo.allowed_tools` → `register_skills` (`registration.rs:245-276`, rejected if a name is unknown) → `CommandContext::Skill.allowed_tools` → `serialize_parsed_command` → `execute.rs:780 stamp_from_mode` → `inner.rs:248-262 narrow(&mut allowed_tools, …)` → `ScopedToolService.allowed` (first retain, `scoped/mod.rs:229-233`). So today a skill's `allowed-tools` **restricts** — the opposite of CC (#34: "does NOT restrict tool access; only pre-grants permission for the listed tools"). Commands keep the restrict wire (#19); skills switch to pre-grant.

**Pre-grant, precisely:** at `resolve_turn_permissions` the merged explicit policy gains an exact-name `Allow` entry for each pre-granted tool **that no explicit entry already binds** (`resolve_explicit(name).is_none()`, `tool_permissions.rs:108`) — so `[policies.tool_permissions]` denies and globs still win, the `Plan` floor still wins (`effective_permission` rung 0), and `requires_confirmation` tools still card (`check_confirmation_gate` reads that independently). What it removes is exactly the tier's `Ask` for the listed names: the CC sentence "your permission settings still govern tools that are not listed".

- [ ] **Step 1: Write the failing tests**

`slash_skill_scope.rs` `mod tests`:

```rust
    #[test]
    fn pregrant_is_a_separate_key_from_the_restrict_scope() {
        let mut md = HashMap::new();
        stamp_pregrant_list(&mut md, &["grep".to_string(), "bash".to_string()]);
        assert!(md.contains_key(SLASH_SKILL_PREGRANT_TOOLS_KEY));
        assert!(!md.contains_key(SLASH_SKILL_ALLOWED_TOOLS_KEY), "pre-grant must not narrow");
        assert_eq!(pregrant_from_metadata(&md), vec!["grep".to_string(), "bash".to_string()]);
        // Absent / empty → nothing pre-granted (an empty pre-grant is a no-op, not deny-all).
        assert!(pregrant_from_metadata(&HashMap::new()).is_empty());
    }

    #[test]
    fn pregrant_entries_are_filtered_to_bare_aleph_names() {
        // `Bash(gh:*)` would pre-grant ALL of bash for a scoped grant: dropped.
        let mut md = HashMap::new();
        stamp_pregrant_from_names(&mut md, &["Bash(gh pr view:*)".to_string(), "Read".to_string(), "grep".to_string()]);
        assert_eq!(pregrant_from_metadata(&md), vec!["file_read".to_string(), "grep".to_string()]);
    }
```

`turn_permissions.rs` `mod tests`:

```rust
    #[test]
    fn a_pregrant_lifts_the_tier_ask_but_never_an_explicit_deny() {
        use crate::config::types::policies::{effective_permission, ExecTier, ToolFacts, ToolPermissionsConfig};
        use crate::extension::PermissionAction;
        let mut merged = ToolPermissionsConfig { default: PermissionAction::Allow, overrides: HashMap::new() };
        merged.overrides.insert("bash".into(), PermissionAction::Deny);
        apply_pregrant(&mut merged, &["bash".to_string(), "file_write".to_string()]);
        let mutating = |name| ToolFacts { name, idempotent: false, requires_approval: false };
        // Explicit deny survives the pre-grant.
        assert_eq!(effective_permission(Some(&merged), Some(ExecTier::Ask), mutating("bash")), PermissionAction::Deny);
        // The tier's Ask is lifted for the pre-granted mutating tool …
        assert_eq!(effective_permission(Some(&merged), Some(ExecTier::Ask), mutating("file_write")), PermissionAction::Allow);
        // … and not for an unlisted one.
        assert_eq!(effective_permission(Some(&merged), Some(ExecTier::Ask), mutating("file_edit")), PermissionAction::Ask);
        // Plan's refusal is a floor, not an Ask: still denied.
        assert_eq!(effective_permission(Some(&merged), Some(ExecTier::Plan), mutating("file_write")), PermissionAction::Deny);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib execution_engine::slash_skill_scope -- --nocapture`
Expected: compile errors (`SLASH_SKILL_PREGRANT_TOOLS_KEY`, `stamp_pregrant_*`, `pregrant_from_metadata`, `apply_pregrant` missing).

- [ ] **Step 3: Write the implementation**

`slash_skill_scope.rs` — add beside the existing key:

```rust
/// Request-metadata key: tools a `/<skill>` PRE-GRANTS for this turn (Claude
/// Code semantics for a skill's `allowed-tools`: no confirmation for these
/// names; the tool SURFACE is untouched). Distinct from
/// [`SLASH_SKILL_ALLOWED_TOOLS_KEY`], which narrows the surface (a
/// command's `allowed-tools`). The two semantics share one field name
/// upstream and must not share a key here.
pub(crate) const SLASH_SKILL_PREGRANT_TOOLS_KEY: &str = "slash_skill_pregrant_tools";

/// Write the pre-grant list (already Aleph names). Empty → nothing written.
pub(crate) fn stamp_pregrant_list(metadata: &mut HashMap<String, String>, tools: &[String]) {
    if tools.is_empty() { return; }
    if let Ok(encoded) = serde_json::to_string(tools) {
        metadata.insert(SLASH_SKILL_PREGRANT_TOOLS_KEY.to_string(), encoded);
    }
}

/// Write the pre-grant list from the names as the skill wrote them: CC names
/// map to Aleph names; scoped `Bash(...)` entries are DROPPED (pre-granting
/// all of `bash` for a scoped grant widens an approval skip).
pub(crate) fn stamp_pregrant_from_names(metadata: &mut HashMap<String, String>, names: &[String]) {
    let tools: Vec<String> = names
        .iter()
        .filter_map(|n| crate::extension::hooks::normalize_cc_tool_entry(n, false))
        .filter(|n| n != "*")
        .collect();
    stamp_pregrant_list(metadata, &tools);
}

/// Read the pre-grant list back. Absent or unreadable → empty (a pre-grant
/// that cannot be read is simply not granted — the fail-closed direction).
pub(crate) fn pregrant_from_metadata(metadata: &HashMap<String, String>) -> Vec<String> {
    metadata
        .get(SLASH_SKILL_PREGRANT_TOOLS_KEY)
        .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
        .unwrap_or_default()
}
```

`execute.rs:776-782` — replace the `stamp_from_mode` call with the kind split (P4.7c's block already lives here; the two compose):

```rust
                if mode.get("type").and_then(|v| v.as_str()) == Some("skill") {
                    let names: Vec<String> = mode.get("allowed_tools").and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                        .unwrap_or_default();
                    match super::slash_command_body::resolve_command_turn(&mode, request.workspace_override.as_deref()).await {
                        // A plugin COMMAND: its `allowed-tools` RESTRICTS this turn
                        // (the first retain in `ScopedToolService::list`).
                        Some(turn) => {
                            super::slash_skill_scope::stamp_from_mode(&mut request.metadata, &mode);
                            request.metadata.insert(super::SLASH_COMMAND_BODY_KEY.to_string(), turn.block);
                            request.model_override = super::slash_command_body::model_override_for(request.model_override.as_ref(), turn.model.as_deref());
                        }
                        // A SKILL: its `allowed-tools` PRE-GRANTS (Claude Code #34) —
                        // the surface stays whole, the listed names skip the tier's Ask.
                        None => super::slash_skill_scope::stamp_pregrant_from_names(&mut request.metadata, &names),
                    }
                }
```

`turn_permissions.rs` — a free function next to `resolve_exec_tier`, called in `resolve_turn_permissions` right after the channel-layer merge (`:305-316`) and before `is_all_default`:

```rust
/// Fold a `/<skill>` turn's pre-granted names into the merged explicit
/// policy as exact-name `Allow` entries — only where no explicit entry
/// (exact or glob) already binds the name, so an operator's deny still
/// wins. The `Plan` floor (`effective_permission` rung 0) and a tool's own
/// `requires_confirmation` gate are untouched: this lifts the TIER's Ask and
/// nothing else, which is what Claude Code's `allowed-tools` on a skill does.
pub(super) fn apply_pregrant(merged: &mut ToolPermissionsConfig, pregrant: &[String]) {
    for name in pregrant {
        if merged.resolve_explicit(name).is_none() {
            merged.overrides.insert(name.clone(), crate::extension::PermissionAction::Allow);
        }
    }
}
```

and in `resolve_turn_permissions`:

```rust
        let pregrant = super::slash_skill_scope::pregrant_from_metadata(&request.metadata);
        if !pregrant.is_empty() {
            apply_pregrant(&mut merged, &pregrant);
            info!(run_id = %request.run_id, pregrant = ?pregrant, "Skill allowed-tools pre-granted for this turn");
        }
```

Add `SLASH_SKILL_PREGRANT_TOOLS_KEY` to the strip/resume handling wherever `SLASH_SKILL_ALLOWED_TOOLS_KEY` is stripped (`slash_skill_scope::strip`; `resume_coordinator.rs:601-690` replays the RESTRICT list from the envelope — the pre-grant is per-turn and deliberately NOT replayed on resume: a resume must only tighten, 判据 §14).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib execution_engine -- --nocapture && cargo test -p alephcore --lib gateway::resume_coordinator -- --nocapture`
Expected: PASS. `session::events::tests::the_envelope_carries_exactly_the_published_knob_keys` stays green (no new knob key: the pre-grant key is not part of `RUN_ENVELOPE_KNOB_KEYS`).

**Mutation step:** in `apply_pregrant` drop the `resolve_explicit(name).is_none()` guard → `a_pregrant_lifts_the_tier_ask_but_never_an_explicit_deny` red on `bash`. Revert.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/execution_engine/slash_skill_scope.rs src/gateway/execution_engine/execute.rs src/gateway/execution_engine/turn_permissions.rs
git commit -m "gateway: a skill's allowed-tools pre-grants; a command's restricts

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P4.12: Skills — Claude-Code-only frontmatter fields are tolerated (`when_to_use` read under both spellings)

**Files:**
- Modify: `src/skill/manifest.rs:107-147` (`RawFrontmatter.when_to_use` alias)
- Test: `src/skill/manifest.rs` `mod tests`

Verified: `RawFrontmatter` (`manifest.rs:104-147`) has NO `deny_unknown_fields`, so `context`, `hooks`, `paths`, `disallowed-tools`, `agent`, `background`, `effort`, `shell`, `arguments`, `metadata`, `license`, `compatibility` already parse silently. One real gap: the container is `rename_all = "kebab-case"`, so Aleph reads `when-to-use` and CC writes `when_to_use` (snake) — the CC spelling is currently ignored. One `#[serde(alias = "when_to_use")]` closes it.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn claude_code_only_frontmatter_parses_and_when_to_use_is_read_under_both_spellings() {
        let dir = tempfile::tempdir().unwrap();
        let skill = dir.path().join("cc-skill");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: cc-skill\ndescription: Does things\nwhen_to_use: when asked\n\
             argument-hint: \"[thing]\"\narguments: [thing]\ndisable-model-invocation: false\n\
             user-invocable: true\nallowed-tools: Read, Grep\ndisallowed-tools: [Write]\nmodel: sonnet\n\
             effort: high\ncontext: fork\nagent: general-purpose\nbackground: false\n\
             hooks:\n  PreToolUse:\n    - matcher: Write\n      hooks: [{type: command, command: echo}]\n\
             paths: [\"src/**\"]\nshell: bash\nmetadata: {author: x}\nlicense: MIT\ncompatibility: \">=1\"\n---\n\
             Body.\n",
        )
        .unwrap();
        // `parse_skill_file` (`manifest.rs:333`) → `SkillManifest`; accessors
        // live on `domain::skill::SkillManifest` (`name` :564, `when_to_use`
        // :636, `allowed_tools` :656).
        let manifest = parse_skill_file(skill.join("SKILL.md"), crate::domain::skill::SkillSource::Global)
            .expect("every CC-only key is tolerated");
        assert_eq!(manifest.name(), "cc-skill");
        assert_eq!(manifest.when_to_use(), Some("when asked"), "snake_case `when_to_use` is Claude Code's spelling");
        assert_eq!(manifest.allowed_tools(), Some(&["Read".to_string(), "Grep".to_string()][..]));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib skill::manifest::tests::claude_code_only -- --nocapture`
Expected: FAIL on `when_to_use == Some("when asked")` (got `None`); everything else parses (proving tolerance was already true).

- [ ] **Step 3: Write the implementation**

`manifest.rs` `RawFrontmatter`:

```rust
    /// Aleph spells it `when-to-use` (the container's kebab-case); Claude
    /// Code spells it `when_to_use`. Both are read.
    #[serde(default, alias = "when_to_use")]
    when_to_use: Option<String>,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib skill::manifest -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/skill/manifest.rs
git commit -m "skill: tolerate Claude Code-only SKILL.md frontmatter; read when_to_use under both spellings

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

**DEVIATION sentence for P8 docs (`PLUGIN_SYSTEM.md`, same table as P4.6):** *"`skills/*/SKILL.md` Claude-Code-only fields (`when_to_use`, `argument-hint`, `arguments`, `disallowed-tools`, `model`, `effort`, `context: fork`, `agent`, `background`, `hooks`, `paths`, `shell`) parse without error and are NOT honoured, except `disable-model-invocation`, `user-invocable`, `allowed-tools` (pre-grant, P4.11) and `when_to_use` (read). A skill relying on `context: fork` runs inline; one relying on skill-scoped `hooks` gets none."*

---
### Task P4.15: A CC `plugin.json` with `mcpServers` / `.mcp.json` is `PluginKind::Mcp` (R4.5)

**Ordering note:** numbered P4.15 by the reconciliation round but executed BEFORE P4.13 — the `cc-cache` QA stage asserts through it.

**Files:**
- Modify: `src/extension/manifest/cc_plugin_json.rs:186-224` (kind inference when there is no `aleph` block)
- Test: `src/extension/manifest/cc_plugin_json.rs` `mod tests`

**Interfaces:**
- Consumes: `CcPluginJson.mcp_servers: Option<ComponentSource>` (`cc_plugin_json.rs:74`), `default_entry_for_kind` (`:141-147`).
- Produces: nothing new — `PluginManifest.kind == PluginKind::Mcp` for a plain Claude Code MCP plugin.

Today (`cc_plugin_json.rs:222-224`):

```rust
    } else {
        (PluginKind::Static, ".".to_string(), None, Vec::new())
    };
```

— a manifest without `"aleph": {"runtime": "mcp"}` is `Static` whatever it declares, and P1.9's `lifecycle.rs::mount_parsed` keys the `mcp_server` effect on `kind == Mcp` (the `// 3. mcp_server — keyed on the manifest's kind` block; `kind` comes from `build_record`'s `parse_manifest_from_dir_cached_global`, which runs THIS adapter), so a pure Claude Code MCP plugin's servers never mount. Nothing in `lifecycle.rs` changes: the fix is upstream, in the one place the kind is decided. The `manifest` QA stage's `qa-inline` fixture (`plant_plugins.py:36-60`, inline `mcpServers` pointing at `echo`) passes today only because its server is never started; P1.16's `plant_scope.py` had to add `"aleph": {"runtime": "mcp"}` by hand for the same reason (plan-P1 P1.16 fixture doc).

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn a_claude_code_manifest_with_mcp_servers_is_an_mcp_plugin_without_an_aleph_block() {
        // Inline object form (two of Anthropic's own plugins) …
        let content = r#"{"name": "cc-mcp", "mcpServers": {"srv": {"command": "node", "args": ["s.js"]}}}"#;
        let manifest = parse_cc_plugin_json_content(content, &test_dir()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Mcp);
        assert_eq!(manifest.entry, PathBuf::from(".mcp.json"));
        // … and the path-string form.
        let content = r#"{"name": "cc-mcp-path", "mcpServers": "./servers.json"}"#;
        let manifest = parse_cc_plugin_json_content(content, &test_dir()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Mcp);
    }

    #[test]
    fn a_plugin_root_with_a_dot_mcp_json_is_an_mcp_plugin() {
        // The `.mcp.json`-at-root convention (context7 on this machine): the
        // manifest says nothing, the file beside it does.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".mcp.json"), r#"{"mcpServers": {"ctx": {"type": "http", "url": "https://x"}}}"#).unwrap();
        let manifest = parse_cc_plugin_json_content(r#"{"name": "ctx7"}"#, dir.path()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Mcp);
    }

    #[test]
    fn a_manifest_without_servers_stays_static_and_an_aleph_block_still_wins() {
        let manifest = parse_cc_plugin_json_content(r#"{"name": "plain"}"#, &test_dir()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Static);
        // An explicit runtime is the author's word: `mcpServers` + `runtime: "wasm"` stays Wasm.
        let content = r#"{"name": "w", "mcpServers": {"s": {"command": "x"}}, "aleph": {"runtime": "wasm"}}"#;
        assert_eq!(parse_cc_plugin_json_content(content, &test_dir()).unwrap().kind, PluginKind::Wasm);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib extension::manifest::cc_plugin_json -- --nocapture`
Expected: the first two FAIL with `left: Static, right: Mcp`; the third passes (control).

- [ ] **Step 3: Write the implementation**

Replace the `else` arm at `cc_plugin_json.rs:222-224`:

```rust
    } else if json.mcp_servers.is_some() || plugin_dir.join(".mcp.json").is_file() {
        // A Claude Code MCP plugin declares no `aleph` block — its servers
        // ARE its runtime. Without this inference the manifest parsed as
        // `Static` and `lifecycle::mount` never started a single server
        // (the `mcp_server` effect keys on `kind == Mcp`). The entry stays
        // `.mcp.json`: the registrar reads `mcpServers` from the manifest
        // first and that file second, whichever the author used.
        (PluginKind::Mcp, default_entry_for_kind(PluginKind::Mcp), None, Vec::new())
    } else {
        (PluginKind::Static, ".".to_string(), None, Vec::new())
    };
```

(`json.mcp_servers` must be read BEFORE `json` is partially moved by the `json.aleph` `if let` — bind `let declares_servers = json.mcp_servers.is_some();` above the `if let Some(aleph_val) = json.aleph` and use it in the `else if`.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib extension::manifest -- --nocapture && cargo test -p alephcore --features test-helpers --test plugin_lifecycle_roundtrip -- --nocapture`
Expected: PASS. P1.15's round-trip fixture already carries `"aleph": {"runtime": "mcp"}`; after this task it would pass without it — leave P1.15/P1.16's fixtures as written (an explicit runtime is still honoured), and drop the "`aleph.runtime = "mcp"` is required" sentence from `qa/plugins/plant_scope.py`'s docstring (P1.16) in this commit, since it is no longer true.

**Mutation step:** revert the `else if` → `a_claude_code_manifest_with_mcp_servers_is_an_mcp_plugin_without_an_aleph_block` red. Revert.

- [ ] **Step 5: Commit**

```bash
git add src/extension/manifest/cc_plugin_json.rs qa/plugins/plant_scope.py
git commit -m "manifest: a Claude Code plugin that declares mcpServers or ships .mcp.json is PluginKind::Mcp

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P4.13: `qa/plugins/run.sh` stages `command`, `exit2`, `cc-cache`

**Files:**
- Modify: `qa/plugins/run.sh` (header comment `:3-12`, the `case "$SCENARIO"` at `:142-405`, the `*)` usage line at `:404` — P1.16 adds a `scope)` arm and a line to both; insert after it)
- Create: `qa/plugins/drive_command.py`, `qa/plugins/drive_exit2.py`, `qa/plugins/drive_cc_cache.py`
- Reuse: `qa/plugins/mcp_mock_server.py` (created by P1.16; the `cc-cache` fixture's `.mcp.json` points at it)
- Modify: `qa/README.md:618-625` (three new lines) and the 「每个装置在证明什么」 table at `:1558+` (three rows)

Conventions followed (from `run.sh` + `qa/file_search/run.sh:160-200` + `qa/README.md` 「Why a mock provider」): scratch `HOME`/`ALEPH_HOME` via `qa_redirect_home`; the fake API key + inert config from `busy_input/patch_config.py`; the mock provider `busy_input/mock_anthropic.py` with `request_log` as the only oracle for what the MODEL received; the binary is the worktree's own `target/debug/aleph-server` (`cargo metadata` resolves the shared target dir — nothing to add). Every driver prints `[PASS]`/`[FAIL]` per assertion and exits with the failure count. **Each stage boots its own server** (the `command` and `exit2` stages need the mock; `cc-cache` needs a restart).

- [ ] **Step 1: `run.sh` — three new `case` arms** (insert before `panel)`)

```bash
command)
  # The claim: `/cmd args` puts the command's RENDERED BODY in front of the
  # model. Before this round the body sat in `SkillRegistration.content`,
  # parsed and never read; `/cmd` reached the model as the literal text.
  # The only oracle for "what the model received" is the mock's request
  # log — the persisted history deliberately stays the RAW input (the
  # session title is derived from it), so `chat.history` would be the
  # wrong surface and is asserted the other way round below.
  say "plant a plugin with a command"
  python3 - "$INSTALLED" <<'PY'
import pathlib, sys
root = pathlib.Path(sys.argv[1], "qa-cmd-plugin")
(root / ".claude-plugin").mkdir(parents=True, exist_ok=True)
(root / "commands").mkdir(exist_ok=True)
(root / ".claude-plugin" / "plugin.json").write_text('{"name": "qa-cmd-plugin", "version": "1.0.0"}\n')
(root / "commands" / "greet.md").write_text(
    "---\ndescription: Greet someone\nargument-hint: \"[name]\"\n---\n"
    "Say hello to $1 and mention the token QA_CMD_MARKER_$1 verbatim.\n"
    "Second: ${2:-nobody}. Inline: !`echo QA_INLINE_SHOULD_BE_WITHHELD`\n")
PY
  say "start mock provider"
  # `single-shot`: the turn answers with no tool call; one request in the log
  # per Think is enough to read the messages the model was handed.
  python3 "$BUSY/mock_anthropic.py" "$MOCK_PORT" /etc/hostname single-shot \
    "" "$QA_ROOT/requests.jsonl" >"$QA_ROOT/mock.log" 2>&1 &
  MOCK_PID=$!
  sleep 1
  say "start server"
  start_server || exit 1
  say "drive"
  python3 "$HERE/drive_command.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$QA_ROOT/requests.jsonl" || RC=$?
  kill "$MOCK_PID" 2>/dev/null
  ;;

exit2)
  # The claim: a PreToolUse hook written the Claude Code way —
  # `echo reason >&2; exit 2` — BLOCKS the tool and its stderr reaches the
  # model as the tool result. Before this round the exit code was recorded
  # and never consulted: the hook ran, printed, and the tool ran anyway.
  # The matcher is spelled `Read` (Claude Code's name for `file_read`) so
  # the alias table is on the same wire; a hook that never fires shows up
  # as the probe's CONTENT reaching the model, which the driver names.
  say "plant a user-level hooks.json and pre-approve its command"
  PROBE="$QA_ROOT/probe.txt"
  printf 'QA_PROBE_CONTENT_MUST_NOT_REACH_THE_MODEL\n' > "$PROBE"
  python3 - "$ALEPH_HOME" <<'PY'
import hashlib, json, pathlib, sys, time
home = pathlib.Path(sys.argv[1])
cmd = "echo QA_BLOCK_REASON_policy >&2; exit 2"
(home / "hooks.json").write_text(json.dumps({"hooks": {"PreToolUse": [
    {"matcher": "Read", "hooks": [{"type": "command", "command": cmd}]}]}}, indent=2))
# `ShellHookConsent::fingerprint` = sha256(plugin_name \0 command)[:16]; user
# hooks are tagged `user:global`. Approving here is what the operator would
# do with `aleph hooks test <fp>`; the file shape is `RegistryDoc`.
fp = hashlib.sha256(b"user:global\0" + cmd.encode()).hexdigest()[:16]
(home / "shell-hooks-allowlist.json").write_text(json.dumps({"version": 1, "entries": [{
    "fingerprint": fp, "plugin_name": "user:global", "command": cmd, "event": "BeforeToolCall",
    "status": "approved", "first_seen": int(time.time()), "approved_at": int(time.time())}]}, indent=2))
PY
  python3 - "$PROBE" "$QA_ROOT/spec.json" <<'PY'
import json, sys
json.dump({"name": "file_read", "input": {"path": sys.argv[1]}}, open(sys.argv[2], "w"))
PY
  say "start mock provider"
  python3 "$BUSY/mock_anthropic.py" "$MOCK_PORT" "$PROBE" tool-chain \
    "$QA_ROOT/spec.json" "$QA_ROOT/requests.jsonl" >"$QA_ROOT/mock.log" 2>&1 &
  MOCK_PID=$!
  sleep 1
  say "start server"
  start_server || exit 1
  say "drive"
  python3 "$HERE/drive_exit2.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$QA_ROOT/requests.jsonl" || RC=$?
  kill "$MOCK_PID" 2>/dev/null
  ;;

cc-cache)
  # The claim: a plugin Claude Code installed under ~/.claude/plugins is
  # discovered, listed with origin `claude_cache`, DISABLED until one verb
  # enables it, its MCP server (declared the Claude Code way — a `.mcp.json`
  # beside the manifest, NO `aleph.runtime`, P4.15) mounts on enable, and
  # nothing under ~/.claude is written. $HOME is the scratch root here
  # (qa_redirect_home), so the fixture IS ~/.claude for the server.
  say "plant a Claude Code plugin cache"
  CC_HOME="$HOME/.claude"
  python3 - "$CC_HOME" "$HERE/mcp_mock_server.py" <<'PY'
import json, pathlib, sys
plugins = pathlib.Path(sys.argv[1], "plugins")
mock = pathlib.Path(sys.argv[2]).resolve()   # P1.16's stdio mock (qa/plugins/mcp_mock_server.py)
root = plugins / "cache" / "qa-market" / "qa-cc" / "1.0.0"
(root / ".claude-plugin").mkdir(parents=True, exist_ok=True)
(root / "commands").mkdir(exist_ok=True)
(root / ".claude-plugin" / "plugin.json").write_text('{"name": "qa-cc", "version": "1.0.0", "description": "CC-installed"}\n')
(root / ".mcp.json").write_text(json.dumps({"mcpServers": {"mock": {"command": sys.executable, "args": [str(mock)]}}}))
(root / "commands" / "hello.md").write_text("---\ndescription: hi\n---\nSay hi to $ARGUMENTS.\n")
(plugins / "installed_plugins.json").write_text(json.dumps({"version": 2, "plugins": {
    "qa-cc@qa-market": [{"scope": "user", "installPath": "/somewhere/else", "version": "1.0.0",
                         "installedAt": "2026-01-01T00:00:00.000Z", "lastUpdated": "2026-01-01T00:00:00.000Z"}]}}, indent=2))
PY
  MARK="$QA_ROOT/.mark"; touch "$MARK"; sleep 1
  say "start server"
  start_server || exit 1
  say "drive (discovered, disabled, enable → command + MCP tool appear)"
  python3 "$HERE/drive_cc_cache.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" first || RC=$?
  say "restart (the enable is durable; boot mounts the command and the server again)"
  stop_server
  start_server || exit 1
  python3 "$HERE/drive_cc_cache.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" second || RC=$?
  say "nothing under ~/.claude changed"
  CHANGED="$(find "$CC_HOME" -newer "$MARK" | grep -v '^$' || true)"
  if [ -z "$CHANGED" ]; then echo "  [PASS] no file under $CC_HOME was written"; else echo "  [FAIL] written under ~/.claude:"; echo "$CHANGED"; RC=1; fi
  ;;
```

and the usage line: `echo "unknown scenario '$SCENARIO' (manifest | scaffold | trust | browse | marketplaces | panel | command | exit2 | cc-cache)" >&2; exit 2;;`. Add `MOCK_PID=""` next to `SERVER_PID=""` and `[ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null` inside `cleanup()`.

- [ ] **Step 2: `qa/plugins/drive_command.py`**

```python
#!/usr/bin/env python3
"""`/cmd args` → the rendered body reaches the model; the raw text stays persisted.

Oracle for "reached the model": the mock provider's request log — each line is
one request the server sent, `body.messages` verbatim. The rendered block rides
the transient trailing user message (never persisted), so it shows up there
and NOT in `chat.history`, and both halves are asserted.
"""
import asyncio, json, sys, time
import websockets

URL, LOG = sys.argv[1], sys.argv[2]
BUDGET = 120.0
rc = 0

def check(ok, label, detail=""):
    global rc
    print(f"  [{'PASS' if ok else 'FAIL'}] {label}" + (f" — {detail}" if detail else ""))
    if not ok:
        rc = 1

def user_texts():
    out = []
    try:
        fh = open(LOG)
    except FileNotFoundError:
        return out
    with fh:
        for line in fh:
            if not line.strip():
                continue
            for m in json.loads(line)["body"].get("messages", []):
                if m.get("role") != "user":
                    continue
                c = m.get("content")
                if isinstance(c, str):
                    out.append(c)
                elif isinstance(c, list):
                    out.extend(b.get("text", "") for b in c if isinstance(b, dict))
    return out

def wait_for(pred):
    end = time.monotonic() + BUDGET
    while time.monotonic() < end:
        for t in user_texts():
            if pred(t):
                return t
        time.sleep(0.5)
    return None

async def main():
    async with websockets.connect(URL, max_size=None) as ws:
        n = [0]
        async def call(method, params):
            n[0] += 1
            await ws.send(json.dumps({"jsonrpc": "2.0", "id": n[0], "method": method, "params": params}))
            while True:
                msg = json.loads(await asyncio.wait_for(ws.recv(), timeout=60))
                if msg.get("id") == n[0]:
                    return msg
        await call("connect", {"client": "qa-command", "version": "1"})
        cmds = json.dumps(await call("commands.list", {}))
        check("qa-cmd-plugin:greet" in cmds, "the plugin command is listed", f"payload {len(cmds)}B")
        check("[name]" in cmds, "argument-hint reaches the listing (usage string)")
        sent = await call("chat.send", {"message": "/qa-cmd-plugin:greet World", "channel": "gui:qa-command"})
        check("error" not in sent, "chat.send accepted /greet", json.dumps(sent.get("error", ""))[:200])
        # `chat.send` answers with the conversation it minted (`result.session_key`,
        # `handlers/chat.rs:289`) — the key `chat.history` is read by.
        session_key = (sent.get("result") or {}).get("session_key")

        hit = wait_for(lambda t: "QA_CMD_MARKER_World" in t)
        check(hit is not None, "the rendered body reached the model ($1 substituted)", f"{len(user_texts())} user text(s) seen")
        if hit:
            check("<command name=\"greet\" plugin=\"qa-cmd-plugin\"" in hit, "wrapped as a <command> block", hit[:160])
            check("Second: nobody." in hit, "${2:-nobody} default applied")
            check("QA_INLINE_SHOULD_BE_WITHHELD" not in hit and "not run: pending operator approval" in hit,
                  "an un-approved !`cmd` is withheld with a visible placeholder", hit[:300])
        # The persisted turn is the raw input (session title invariant).
        hist = await call("chat.history", {"session_key": session_key, "limit": 5})
        text = json.dumps((hist.get("result") or {}).get("messages") or [])
        check("/qa-cmd-plugin:greet World" in text, "the persisted user turn is the raw /cmd text")
        check("QA_CMD_MARKER_World" not in text, "the rendered body is NOT persisted")

asyncio.run(main())
print(f"drive_command: rc={rc}")
sys.exit(rc)
```

- [ ] **Step 3: `qa/plugins/drive_exit2.py`**

```python
#!/usr/bin/env python3
"""A `>&2; exit 2` PreToolUse hook blocks the tool and its reason reaches the model."""
import asyncio, json, sys, time
import websockets

URL, LOG = sys.argv[1], sys.argv[2]
BUDGET = 150.0
REASON = "QA_BLOCK_REASON_policy"
PROBE_CONTENT = "QA_PROBE_CONTENT_MUST_NOT_REACH_THE_MODEL"
rc = 0

def check(ok, label, detail=""):
    global rc
    print(f"  [{'PASS' if ok else 'FAIL'}] {label}" + (f" — {detail}" if detail else ""))
    if not ok:
        rc = 1

def tool_results():
    out, seen = [], set()
    try:
        fh = open(LOG)
    except FileNotFoundError:
        return out
    with fh:
        for line in fh:
            if not line.strip():
                continue
            for m in json.loads(line)["body"].get("messages", []):
                c = m.get("content")
                if not isinstance(c, list):
                    continue
                for b in c:
                    if isinstance(b, dict) and b.get("type") == "tool_result":
                        inner = b.get("content")
                        text = inner if isinstance(inner, str) else json.dumps(inner)
                        if text not in seen:
                            seen.add(text)
                            out.append(text)
    return out

def wait_for(pred):
    end = time.monotonic() + BUDGET
    while time.monotonic() < end:
        for t in tool_results():
            if pred(t):
                return t
        time.sleep(0.5)
    return None

async def main():
    async with websockets.connect(URL, max_size=None) as ws:
        await ws.send(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "connect", "params": {"client": "qa-exit2", "version": "1"}}))
        await ws.send(json.dumps({"jsonrpc": "2.0", "id": 2, "method": "chat.send",
                                  "params": {"message": "read the probe file", "channel": "gui:qa-exit2"}}))
        end = time.monotonic() + 30
        while time.monotonic() < end:
            m = json.loads(await asyncio.wait_for(ws.recv(), timeout=30))
            if m.get("id") == 2:
                check("error" not in m, "chat.send accepted", json.dumps(m.get("error", ""))[:200])
                break
    blocked = wait_for(lambda t: REASON in t)
    if blocked is None:
        results = tool_results()
        leaked = any(PROBE_CONTENT in t for t in results)
        check(False, "the exit-2 hook's stderr reached the model as the tool result",
              "the hook did not fire (matcher `Read` → file_read alias broken?) — the probe content reached the model"
              if leaked else f"no result carried the reason; {len(results)} tool_result(s) seen")
        return
    check(True, "the exit-2 hook's stderr reached the model as the tool result", blocked[:200])
    check(all(PROBE_CONTENT not in t for t in tool_results()), "the tool did NOT run (probe content never reached the model)")

asyncio.run(main())
print(f"drive_exit2: rc={rc}")
sys.exit(rc)
```

- [ ] **Step 4: `qa/plugins/drive_cc_cache.py`**

```python
#!/usr/bin/env python3
"""~/.claude/plugins discovery: listed with origin claude_cache, disabled, one verb enables, durable."""
import asyncio, json, sys
import websockets

URL, PHASE = sys.argv[1], sys.argv[2]
rc = 0

def check(ok, label, detail=""):
    global rc
    print(f"  [{'PASS' if ok else 'FAIL'}] {label}" + (f" — {detail}" if detail else ""))
    if not ok:
        rc = 1

async def main():
    async with websockets.connect(URL, max_size=None) as ws:
        n = [0]
        async def call(method, params):
            n[0] += 1
            await ws.send(json.dumps({"jsonrpc": "2.0", "id": n[0], "method": method, "params": params}))
            while True:
                msg = json.loads(await ws.recv())
                if msg.get("id") == n[0]:
                    return msg
        await call("connect", {"client": "qa-cc-cache", "version": "1"})
        rows = {r["name"]: r for r in (await call("plugins.list", {}))["result"]["plugins"]}
        row = rows.get("qa-cc")
        check(row is not None, "the Claude Code-installed plugin is discovered", f"names: {sorted(rows)[:10]}")
        if row is None:
            return
        check(row.get("origin") == "claude_cache", "listed with origin claude_cache", str(row.get("origin")))
        async def catalog_names():
            msg = await call("tools.catalog", {})
            out = set()
            def walk(v):
                if isinstance(v, dict):
                    for k, x in v.items():
                        if k == "name" and isinstance(x, str):
                            out.add(x)
                        walk(x)
                elif isinstance(v, list):
                    for x in v:
                        walk(x)
            walk(msg.get("result", {}))
            return out
        async def poll_catalog(pred, label, seconds=30):
            for _ in range(seconds * 2):
                if pred(await catalog_names()):
                    check(True, label)
                    return True
                await asyncio.sleep(0.5)
            check(False, label, f"catalog names: {sorted(await catalog_names())[:20]}")
            return False
        has_echo = lambda names: any(n.endswith("__qa_echo") for n in names)
        if PHASE == "first":
            check(row["status"] == "disabled" and not row["enabled"], "disabled until Aleph is told otherwise", row["status"])
            check(not has_echo(await catalog_names()), "a disabled plugin's MCP tool is NOT in tools.catalog")
            t = await call("tools.invoke", {"tool_name": "plugin_manage", "arguments": {"action": "enable", "name": "qa-cc"}})
            check("error" not in t, "plugin_manage enable accepted", json.dumps(t.get("error", ""))[:200])
            rows = {r["name"]: r for r in (await call("plugins.list", {}))["result"]["plugins"]}
            check(rows["qa-cc"]["enabled"], "enabled after one verb", rows["qa-cc"]["status"])
            # P1's mount registers the slash entry and starts the server right
            # away — no restart needed for either.
            cmds = json.dumps(await call("commands.list", {}))
            check("qa-cc:hello" in cmds, "the command is registered by the mount (P1.7 slash effect)", f"payload {len(cmds)}B")
            await poll_catalog(has_echo, "the .mcp.json server (no aleph.runtime) mounted: mock__qa_echo is in tools.catalog (P4.15)")
        else:
            check(row["enabled"], "the enable survived a restart (plugins.toml, not ~/.claude)", row["status"])
            cmds = json.dumps(await call("commands.list", {}))
            check("qa-cc:hello" in cmds, "the cached plugin's command is registered after boot", f"payload {len(cmds)}B")
            await poll_catalog(has_echo, "the MCP server mounts again at boot")

asyncio.run(main())
print(f"drive_cc_cache({PHASE}): rc={rc}")
sys.exit(rc)
```

- [ ] **Step 5: Run the three stages on the worktree binary, then record**

Run: `./qa/plugins/run.sh command && ./qa/plugins/run.sh exit2 && ./qa/plugins/run.sh cc-cache`
Expected: every `[PASS]`, `verdict: rc=0` three times. Paste the three `[PASS]/[FAIL]` blocks into the commit message body (the number of assertions is what the fixture prints, not a count written here).

`qa/README.md` additions (`:618-625` block):

```
./qa/plugins/run.sh command      # `/cmd args` → the command's rendered body (with $1 / ${2:-d}
                                 # substituted and an un-approved !`cmd` withheld visibly) is in
                                 # the messages the MODEL received; the persisted turn stays raw
./qa/plugins/run.sh exit2        # a `>&2; exit 2` PreToolUse hook — Claude Code's most common
                                 # idiom, matcher spelled `Read` — blocks file_read and its
                                 # stderr is the tool result the model reads
./qa/plugins/run.sh cc-cache     # ~/.claude/plugins/installed_plugins.json → discovered, origin
                                 # claude_cache, DISABLED, one verb enables it durably, its
                                 # `.mcp.json` server (no aleph.runtime) reaches tools.catalog,
                                 # and nothing under ~/.claude is written (find -newer)
```

- [ ] **Step 6: Commit**

```bash
git add qa/plugins/run.sh qa/plugins/drive_command.py qa/plugins/drive_exit2.py qa/plugins/drive_cc_cache.py qa/README.md
git commit -m "qa/plugins: command, exit2 and cc-cache stages

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task P4.14: Acceptance table — the 70 items of `scan-cc-plugin-format.md`

This is the plan's §10. **IMPLEMENTED** = code + test exist at 3ddc1f2e7 (cite); **CONNECT** = this plan wires it (task id); **DEVIATION** = not honoured, with the reason. Items owned by other phases say so.

| # | Item (short) | Verdict | Evidence / task / reason |
|---|---|---|---|
| 1 | manifest at `.claude-plugin/plugin.json` | IMPLEMENTED | `manifest/cc_plugin_json.rs` detect; tests there |
| 2 | component dirs at plugin root | IMPLEMENTED | `cc_plugin_json.rs` + `declared_sections.rs`; `workflows/`,`output-styles/`,`themes/`,`monitors/` not scanned (DEVIATION: no consumer) |
| 3 | only `name` required; LSP-only entries without a manifest | IMPLEMENTED / DEVIATION | `name` only: `cc_plugin_json.rs`; marketplace-inline LSP plugins: P4.10 skips dirs with no manifest (`discovery_lists_only_cache_dirs_that_carry_a_manifest`) — DEVIATION: no LSP host |
| 4 | `version` opaque (git-sha ok) | IMPLEMENTED | P4.10 treats `version` as a directory name (`parses_the_real_shape_…`); manifest `version` is a free string |
| 5 | full `plugin.json` field set | DEVIATION | parsed: `name/version/description/author/homepage/repository/license/keywords/commands/agents/skills/hooks/mcpServers` (`cc_plugin_json.rs`); ignored: `displayName/metadata/defaultEnabled/$schema/workflows/outputStyles/lspServers/userConfig/channels/dependencies/experimental` (unknown keys tolerated, #6) |
| 6 | unknown top-level keys ignored | IMPLEMENTED | `cc_plugin_json.rs` manifest struct has no `deny_unknown_fields`; test `component_source.rs` lenient cases |
| 7 | type mismatch on a known field fails load | IMPLEMENTED | serde default behaviour; `an_object_source_does_not_take_the_whole_marketplace_down` documents the one deliberate exception (marketplace `source`) |
| 8 | supplements vs replaces per field | DEVIATION | Aleph unions custom paths with defaults for every field (`component_source.rs`); CC replaces for `commands`/`agents`. No producer of a conflict observed on this machine |
| 9 | override paths relative, `./`-prefixed, arrays | IMPLEMENTED | `component_source.rs` (string / array / inline-object) |
| 10 | `${CLAUDE_PLUGIN_ROOT}` everywhere | IMPLEMENTED | `manifest/adapter.rs:108-139`, `plugin_vars.rs`; tests `plugin_variables_are_expanded_in_skill_prose` |
| 11 | `${CLAUDE_PLUGIN_DATA}` | IMPLEMENTED | `hooks/mod.rs substitute_variables`, `executor.rs:583-586` |
| 12 | `userConfig` → `$CLAUDE_PLUGIN_OPTION_*` | DEVIATION | Aleph's per-plugin settings are `config_schema` + `plugins.toml` (`plugin_state.rs`), exposed via `settings_env` (`executor.rs:597-600`) under Aleph names; CC's `userConfig` block is not parsed |
| 13 | `channels[]` | DEVIATION | no consumer (Aleph channels are `[channels]` config) |
| 14 | `dependencies[]` | DEVIATION | user ruling 2026-08-19 (defer; FL §3.10 ①) |
| 15 | `lspServers` | DEVIATION | no LSP host |
| 16 | command body = instructions to the agent | **CONNECT** | P4.7a–c; the body rides the transient user message, the persisted turn is the raw `/cmd` (open question) |
| 17 | three command scopes + labels | IMPLEMENTED / DEVIATION | project `.claude/commands` + `~/.claude/commands` are scanned (`scanner.rs:88-160`); `/help` scope labels are not rendered (DEVIATION, cosmetic) |
| 18 | command frontmatter fields | **CONNECT** / DEVIATION | `description`,`argument-hint`,`allowed-tools` (both forms) — P4.7b; `model` — P4.7c only for a routable model id (bare `sonnet/opus/haiku` ignored: no catalog alias facility); `disable-model-invocation` — parsed, no effect (P4.7e: commands are structurally human-only) |
| 19 | command `allowed-tools` restricts | **CONNECT** | P4.7b + P4.11 (the existing `slash_skill_scope` retain, now fed for commands); scoped `Bash(...)` folds to bare `bash` (DEVIATION: no per-argument scoping) |
| 20 | `$ARGUMENTS`, `$1…$N` | **CONNECT** | P4.7a (`positional_and_default_arguments`); whitespace split, no quoting (DEVIATION note) |
| 21 | `@path` / `@$1` | IMPLEMENTED / CONNECT | `template.rs` `@./` relative, jailed to the command's dir (absolute paths refused — DEVIATION vs CC); `@$1` works because args substitute first (P4.7a order) |
| 22 | `` !`cmd` `` | **CONNECT** | P4.7a + P4.7c `ConsentedShell` (consent-gated, 30 s, 64 KiB); DEVIATION: requires operator approval per `(plugin, command)`, not `Bash` in `allowed-tools` |
| 23 | command namespacing by subdirectory | DEVIATION | `scan_component_dir` is one level (`parsers.rs:250-290`); `plugin:name` only |
| 24 | agent frontmatter `name/description/model/color/tools` | IMPLEMENTED / **CONNECT** / DEVIATION | `name/description/model` (`parsers.rs:64-71`); `tools` + `color` — P4.8; `permissionMode` — DEVIATION (R4.1 Q6: a sub-agent runs on its parent's tier, `allowlist_tool_service.rs:118-121`; parsed and logged, never applied) |
| 25 | long block-scalar `description` | IMPLEMENTED | YAML block scalars parse (`crate::yaml`); no length cap |
| 26 | agent body = system prompt, 10k soft ceiling | **CONNECT** | P4.8 (`body_to_system_prompt`, warn over 10 000 chars, kept whole) |
| 27 | agent namespacing | DEVIATION | flat `plugin:name` |
| 28 | `skills/<name>/SKILL.md` discovery | IMPLEMENTED | `parse_skills_dir`, `projection.rs:82` |
| 29 | only `name`+`description` required | IMPLEMENTED | `skill/manifest.rs:104-108` |
| 30 | third-person description not enforced | IMPLEMENTED | no linter on the load path |
| 31 | CC-only skill fields | **CONNECT** (tolerance test) / DEVIATION | P4.12 — parse, not honoured (list in P4.12's sentence) |
| 32 | progressive disclosure | IMPLEMENTED | `<available_skills>` + `skill_read` (`thinker::layers::skill_instructions`) |
| 33 | `context: fork` / `agent` / `background` | DEVIATION | P4.12 sentence |
| 34 | skill `allowed-tools` pre-grants, does not restrict | **CONNECT** | P4.11 (`a_pregrant_lifts_the_tier_ask_but_never_an_explicit_deny`) |
| 35 | skill-scoped `hooks` | DEVIATION | P4.12 sentence |
| 36 | Agent-Skills-spec subset for publishing | DEVIATION | Aleph does not publish skills to claude.ai |
| 37 | plugin `hooks.json` wrapper vs settings shape | IMPLEMENTED | `parsers.rs parse_hooks_file` (wrapped); `hooks/user_settings.rs` accepts the same wrapper for `~/.aleph/hooks.json` |
| 38 | hook entry fields `shell`/`async`/`statusMessage` | IMPLEMENTED (tolerated) | `parsers.rs:94` `HookAction` has no `deny_unknown_fields`; not honoured (DEVIATION: `async` — Aleph observers are already parallel; `shell` — always `sh`/`cmd`) |
| 39 | minimum event set incl. `PermissionRequest`, `PostToolUseFailure`; unknown events no-op | IMPLEMENTED / **CONNECT** | 9 + 2 present (`types/hooks.rs`); `PermissionDenied` + `PostCompact` — P4.4; unknown names skipped with warn (`user_settings.rs`) |
| 40 | hook types `command` + `prompt` (+`http`/`mcp_tool`/`agent`) | IMPLEMENTED / DEVIATION | `HookAction::{Command,Prompt,Http,Agent,Plugin}`; CC `prompt` hooks are NOT LLM-judged in Aleph (the prompt becomes context — R7/R10), `mcp_tool` absent |
| 41 | `matcher` regex, case-sensitive, MCP patterns | IMPLEMENTED / **CONNECT** | `executor.rs:311-340`; CC spellings (`Edit|Write`, `mcp__…`) — P4.5 |
| 42 | stdin envelope | **CONNECT** / DEVIATION | `transcript_path`, `permission_mode`, `cwd` fallback — P4.3; `hook_event_name` echoes the spelling the hook was registered under (`PreToolUse` for a CC-style file, `before_tool_call` for an Aleph one) — **CONNECT** P4.4 (user ruling U-b); `prompt_id`/`scratchpad_dir`/`effort`/`agent_id`/`agent_type` not emitted |
| 43 | `tool_name`/`tool_input`/`tool_use_id`; result key | IMPLEMENTED / **CONNECT** / DEVIATION | `tool_name`,`tool_input` present; result key `CC_POST_TOOL_RESULT_KEY` (P0 → P4.3); `tool_use_id` not emitted |
| 44 | `user_prompt`, `reason` | DEVIATION | UserPromptSubmit carries the prompt as `tool_input` (`inner.rs:406`), not `user_prompt`; Stop carries no `reason` key |
| 45 | `$TOOL_INPUT` etc. in prompt hooks | IMPLEMENTED | `substitute_variables` (`hooks/mod.rs`) |
| 46 | universal stdout envelope `continue/suppressOutput/systemMessage` | IMPLEMENTED / DEVIATION | `continue`,`systemMessage` (`json_output.rs`); `suppressOutput` ignored |
| 47 | `hookSpecificOutput.{permissionDecision,updatedInput,…}` | IMPLEMENTED / **CONNECT** / DEVIATION | `permissionDecision` (allow/deny/ask), `permissionDecisionReason`, `additionalContext` present; `updatedInput` and `permissionDecision: "block"` (live-docs spelling → retryable Block) — P4.2 (R4.1 Q5); `hookEventName` not validated, `terminalSequence`/`retry` ignored |
| 48 | Stop `{decision: approve\|block, reason}` | IMPLEMENTED | `json_output.rs` legacy arm; `extension_stop_gate.rs` |
| 49 | exit-code semantics | **CONNECT** | P4.1 (`derive_decision`); `WorktreeCreate/Remove` exception n/a |
| 50 | default timeouts 600 s | DEVIATION | P4.6 sentence (300 s default and ceiling) |
| 51 | env vars | IMPLEMENTED / **CONNECT** / DEVIATION | `CLAUDE_PLUGIN_ROOT`, `CLAUDE_PLUGIN_DATA`, plugin settings env present; `CLAUDE_PROJECT_DIR` — P4.3; `CLAUDE_ENV_FILE`, `CLAUDE_CODE_REMOTE`, `CLAUDE_CODE_BRIDGE_SESSION_ID`, `CLAUDE_EFFORT`, `CLAUDE_PLUGIN_OPTION_*` not set |
| 52 | matching hooks run in parallel | IMPLEMENTED / DEVIATION | observers parallel (`execute_observers`); interceptors sequential by priority (DEVIATION by design: an interceptor chain threads `updated_input`) |
| 53 | plugin + user hooks merge | IMPLEMENTED | `sync_user_hooks` layers on `sync_hooks_from_registry` (`mod.rs:1082+`) |
| 54 | hooks load at session start; invalid JSON is a load failure | DEVIATION | Aleph hot-reloads (`watcher.rs`, `hooks.reload`) and fails soft on a malformed file (`user_settings.rs` "fail-soft") — deliberate, P7 |
| 55 | `.mcp.json` or inline `mcpServers` | IMPLEMENTED / **CONNECT** | parsing: `registrar/mcp_registrar.rs`, `mcp_config.rs`; **kind**: a CC manifest without `aleph.runtime` parsed as `Static` and its servers never mounted — P4.15 infers `Mcp` from `mcpServers` / `.mcp.json` (R4.5), proven on a daemon by the `cc-cache` stage |
| 56 | server shapes stdio/sse/http/ws | IMPLEMENTED / DEVIATION | `McpServerConfig::{Stdio,Remote}` (`types/hooks.rs:392-420`): `http`/`sse` map to `Remote`; `ws` not supported; CC-side OAuth-in-browser n/a |
| 57 | `${VAR:-}` default expansion in `.mcp.json` | DEVIATION | `mcp_config.rs:259-265 substitute_vars` expands exactly the four plugin variables (`CLAUDE_/ALEPH_PLUGIN_{ROOT,DATA}`); neither `${ENV_VAR}` nor `${VAR:-default}` is expanded from the process environment (an operator's per-plugin settings reach an MCP server's env through `plugin_vars::settings_env`, under Aleph's names). A `context7`-style `${CONTEXT7_API_KEY:-}` header stays literal |
| 58 | MCP tool naming `mcp__plugin_<p>_<s>__<t>` | DEVIATION / **CONNECT** | Aleph names are `<server>__<tool>` (`tools/handlers/mcp.rs:103`); matchers accept `mcp__<server>__<tool>` (P4.5); the plugin-prefixed spelling needs `mcp__.*` |
| 59 | `allowed-tools` MCP wildcards accepted syntactically | **CONNECT** | P4.5 `normalize_cc_tool_entry` (`mcp__x_y__*` → `y__*`; the registry then rejects an unknown glob — DEVIATION: globs are not expanded) |
| 60 | `/mcp`-style introspection | IMPLEMENTED | `plugin_manage show` + `mcp.*` RPCs (P5 decides the RPC set) |
| 61 | `.claude/<plugin>.local.md` convention | IMPLEMENTED (nothing to do) | a plugin's own hooks read it with `sed`; Aleph does not parse it (same as CC) |
| 62 | `enabledPlugins` composite key | DEVIATION | user ruling U6: `settings.json` is not read; enable state is `plugins.toml` keyed by Aleph plugin id (P4.10); `installed_plugins.json` composite keys are parsed for discovery only |
| 63 | `extraKnownMarketplaces` | DEVIATION | U6 |
| 64 | `marketplace.json` top level incl. `renames` | IMPLEMENTED / DEVIATION | `marketplace/types.rs` (`name/owner/metadata/plugins`); `renames` ignored |
| 65 | per-plugin marketplace entry fields | IMPLEMENTED / DEVIATION | `MarketplacePluginEntry` (`name/source/description/version/sha256/…`); `displayName/strict/skills[]/lspServers/defaultEnabled/tags` ignored (unknown keys tolerated) |
| 66 | `source.source` AND `source.type` | **CONNECT** | P4.9 |
| 67 | source variants | IMPLEMENTED / DEVIATION | path form installs; every object form parses and refuses by name at install (`installable_path`) — `github/git/git-subdir/url/npm/local/command/archive/synced` are NOT fetched from a marketplace entry (add the repo as its own marketplace) |
| 68 | `known_marketplaces.json` | DEVIATION | not read (Aleph keeps its own `[plugin_marketplaces]`) |
| 69 | `installed_plugins.json` schema v2, array per key | **CONNECT** | P4.10 (`parse_installed_plugins`; every array element is a candidate; `scope` ignored) |
| 70 | commands are first-class beside skills | **CONNECT** | P4.7 (the command body is finally delivered; listed via `commands.list`) |

dsh / pi-mcp-adapter one-row mounts (spec §10 last paragraph) belong to P6's MCP face and are not in this table.

---

## Contract deltas

- `derive_decision` takes a `&mut HookResult` accumulator (the existing decision type is filled by `&mut`, last-writer-wins across an interceptor chain — `parse_command_output`'s shape); it returns nothing. Signature: `derive_decision(exit_code: Option<i32>, stdout: &str, stderr: &str, kind: HookKind, result: &mut HookResult)`.
- `CC_TOOL_ALIASES` lives in `src/extension/hooks/cc_tool_aliases.rs` and has THREE readers (hook matcher, command `allowed-tools`, agent `tools`), not one: `register_skills` refuses any unknown name, so the command and agent parsers must map CC names before registration or every CC plugin's commands are rejected.
- `TemplateCtx` is `TemplateCtx<'a> { shell: Option<&'a dyn InlineShell> }` — no `cwd`, no `file_reader`: `SkillTemplate` already owns a jailed async `@file` reader keyed on its own base dir; `cwd` belongs to the shell runner (`ConsentedShell`). `render(&self, args: &str, ctx: &TemplateCtx<'_>) -> ExtensionResult<String>` (the crate's `ExtensionResult`, not `Result<String, TemplateError>`).
- `PluginId` is a bare `String` everywhere this phase touches (`SkillRegistration.plugin_id`, `PluginRecord.id`, `plugin_state` keys).
- `PluginOrigin::ClaudeCache` rows must be `ScopeKey::Global` in P2's derivation (P2 owns `ScopeKey`; P4.10 adds the variant and `classify`).
- P1's `mount` must call `PluginsConfig::is_enabled_for(id, origin)` (P4.10), not `is_enabled(id)`, and must skip the legacy `.disabled` migration for `ClaudeCache`; `set_plugin_enabled(id, true)` → `mount` is unchanged.
- P4.7d CUTs only `plugins.executeCommand` (+ `execute_plugin_command`, `PluginLoader::execute_command`, `DirectCommandResult`, `ExecuteCommandParams`); `plugins.{load,unload}` remain P5's.
- `PluginRow` (wire, `shared/protocol`) gains `#[serde(default)] origin: String` — Panel / TUI / CLI decode it with a default; no client change required (P2/P3 Panel work may render it).
- `HookEvent::ALL` grows from 23 to 24 (`PermissionDenied`); `hooks_manage` and `hooks.events` derive from it.
- `HookContext` gains `transcript_path: Option<PathBuf>` and `permission_mode: Option<&'static str>` (the contract listed only payload keys).
- `SkillRegistration` gains `argument_hint`, `allowed_tools` (Aleph names), `model`; `SkillInfo` gains `argument_hint`; `AgentDef` gains `system_prompt`; `DiscoveryConfig` gains `claude_home_override: Option<PathBuf>` (test seam).
- The command body is delivered as a transient user message (`SLASH_COMMAND_BODY_KEY` → `transient_blocks`), not as the persisted user turn (user ruling U-c).
- `HookRegistration` and `HookConfig` gain `declared_event: Option<String>`; `HookConfig::event_name()` is the one derivation of the payload's `hook_event_name` (U-b / R4.2); `HooksFileConfig.hooks` is keyed by `String`, and `user_settings::parse_event` is the shared spelling parser (`pub(crate)`).
- P4.10 edits P1.9's `lifecycle.rs::admit` (`is_enabled` → `is_enabled_for(id, origin)`) and P1.9's `load_all` marker-migration call (skipped for `ClaudeCache`), and adds `GlobalRoot::ClaudeCache` to P2.3's `DiscoveryScope` (`source()` arm → `DiscoverySource::ClaudeCache`; `visibility.rs::from_discovery` arm joined into the `Global` pattern — the match is wildcard-free, so the compiler is the guard) — the 3ddc1f2e7 anchors `mod.rs:630-676` are history (R4.3). ClaudeCache discovery enters through `discover_plugins_with_extra` as `DiscoveredPath::global(dir, GlobalRoot::ClaudeCache, 5)`, not through a `ScanDirectory` (that type feeds component scans, not the JSON-indexed cache).
- P4.7b edits P1.7's `slash_effect::plugin_command_skill_infos` (projects `allowed_tools` + `argument_hint`), its test and the "always `None`" comment in `register_slash_commands_effect`; `tool_catalog_init.rs:214-247` is deleted by P1.10 (R4.4).
- P4.7d also owns the `EXTENSION_SYSTEM.md:613-709` "Direct Commands" replacement (identical text to P5.3; whichever lands first writes it) AND the second edit of `PLUGIN_SYSTEM.md:481-483` (P1.10 wrote the true-at-P1.10 wording that still names `executeCommand`; P4.7d drops it — lead addendum to R4.4).
- New task P4.15 (R4.5): `cc_plugin_json.rs` infers `PluginKind::Mcp` from `mcpServers` / `.mcp.json`; P1.15/P1.16 fixtures' explicit `aleph.runtime` stays valid; `plant_scope.py`'s "required" sentence is corrected in P4.15's commit.
- `PluginStatus` names are today's (G-2): this file uses `disabled` / `loaded` / `blocked` / `error` only.

## Open questions for the lead

All eight questions of the first draft were answered by the reconciliation round (R4.1 / R4.2 / G-7 / G-8) and are applied above:

1. P0 stays the pre-step of P4.3 (`CC_POST_TOOL_RESULT_KEY` = `tool_response` until the capture says otherwise).
2. `hook_event_name` = the spelling the hook was registered under (U-b) — P4.4 `declared_event` / `HookConfig::event_name()`; row 42 is CONNECT.
3. `ExecTier::Auto → "auto"` (G-8).
4. `/cmd`: raw text persisted, body delivered transiently (U-c) — P4.7c as planned.
5. `permissionDecision: "block"` accepted as the retryable Block — P4.2.
6. Agent `permissionMode` = DEVIATION this round — P4.8 + table row 24.
7. The `command` QA stage keeps the mock request-log oracle.
8. No follow-up round for the P4.4 candidate rows; the table is the record.

Nothing remains open from this phase. (An earlier note here about a wildcard in P2.3's `from_discovery` is obsolete: P2.3's final shape matches `GlobalRoot` by name with no wildcard, and P4.10 Step 3 adds the `GlobalRoot::ClaudeCache` arm the compiler demands.)

## Coverage map

- §3.6 #1 (exit 2 = block) → P4.1
- §3.6 #2 (`updatedInput`) → P4.2
- §3.6 #3 (stdin `transcript_path` / `permission_mode` / P0 result key) → P0, P4.3
- §3.6 #4 (32 events) → P4.4 (table + `PostCompact` alias + `PermissionDenied`)
- R4.2 / U-b (`hook_event_name` echoes the registered spelling) → P4.3 (builder keyed on the name) + P4.4 (`declared_event`, test)
- §3.5 G4 (alias census) → P4.4 `producer_census.rs` (4 mutation steps)
- §3.6 #5 (matcher CC names, `mcp__`) → P4.5
- §3.6 #6 (timeout DEVIATION) → P4.6 (doc sentence for P8)
- §3.6 #7 (commands: template, `!cmd` consent, injection, `argument-hint`, `allowed-tools` retain, `model` pin, `disable-model-invocation`, CUT `executeCommand`) → P4.7a, P4.7b, P4.7c, P4.7d, P4.7e
- §3.6 #8 (agents body, `permissionMode` table, `color`) → P4.8
- §3.6 #9 (marketplace discriminator) → P4.9
- §3.6 #10 (`~/.claude/plugins` read-only, `ClaudeCache`, default disabled, origin in list) → P4.10
- §3.6 #11 (`allowed-tools` two semantics) → P4.11 (+ P4.7b for the command half)
- §3.6 #12 (skills CC-only fields) → P4.12
- R4.5 (CC `mcpServers` / `.mcp.json` ⇒ `PluginKind::Mcp`) → P4.15 (+ `cc-cache` stage assertion in P4.13)
- R4.6 (doc-code 同笔) → P4.1 (FL §5.10), P4.9 (`PLUGIN_SYSTEM.md:499`), P4.7d (`EXTENSION_SYSTEM.md:613-709`)
- §5 unit tests (hooks matrix / template / marketplace / ClaudeCache / AgentDef) → P4.1, P4.7a, P4.9, P4.10, P4.8
- §6 `qa/plugins` `command` / `exit2` / `cc-cache` → P4.13
- §10 acceptance table → P4.14
