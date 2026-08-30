# Module: interfaces/tui (review 2026-08-29)

## Summary

- **Files**: 28 `.rs` files + `Cargo.toml`  ·  `build.rs` is not present in this crate
- **Issues**: 5 total (0 critical / 2 high / 2 medium / 1 low)
- **R1 platform-API isolation**: PASS — no platform FFI; only `crossterm`/`ratatui` terminal abstractions
- **R4 Interface layers are pure I/O**: PASS — the TUI maps terminal/gateway events to `Action`, renders results, and forwards slash commands; all business logic lives in the gateway
- **R5 Menu bar / command palette first**: PASS — status bar is always visible and `/` opens the command palette
- **R6 AI comes to you**: PASS — inline `AskUser` dialog, tool-approval overlay, `/btw` side-question panel, and transient system notices
- **R8 No regex for natural language**: PASS — regex is not used for intent/ routing; only present in the markdown/link parser for machine-format text
- **Sync primitives**: N/A — `AppState` is owned and mutated only by the single main loop; no shared locks are needed

## High-Confidence Issues

### [High] ANSI / OSC / control-character injection via bracketed paste

- **Location**: `src/tui/event.rs::map_event`, `src/tui/keys.rs::handle_terminal_event` (Paste arm)
- **Description**: `Event::Paste(text)` is forwarded verbatim into `TextArea::insert_str`. A paste that contains terminal escape sequences (CSI color codes, cursor movement, screen clear) or OSC 52 clipboard requests can alter the user's terminal, and embedded NUL / CR / backspace bytes can leak into the message that is eventually sent to the gateway. Crossterm gives us the raw bytes the terminal reported; sanitizing them is the interface layer's job.
- **Risk / trigger**: Pasting text from an untrusted source (a log file, a web page, a chat message) that contains `\x1b[31m...\x1b[0m` or `\x1b]52;c;...\x07`. The TUI will render it and then transmit it, making the injection part of the conversation.
- **Fix**: Added `event.rs::sanitize_pasted_text` that strips ANSI CSI/OSC sequences and drops NUL, CR, DEL, and other C0 controls while preserving newline and tab. The sanitizer runs at the single producer of `TermEvent::Paste`, so every consumer gets clean text. Added unit tests for CSI color codes, OSC 52, NUL/CR, and tab/newline preservation.
- **Decision**: FIXED.

### [High] `send_history` grows without bound

- **Location**: `src/tui/mod.rs` (`Action::SendMessage`, `Action::GatewayCommand`), `src/tui/commands.rs::execute_retry`, `src/tui/app/mod.rs::send_history`
- **Description**: Every message and every slash command is pushed onto `AppState.send_history` with no cap. In a long-lived TUI session this is an unbounded memory leak and a known bug pattern for this crate.
- **Risk / trigger**: A user who keeps the same TUI open for a multi-thousand-message session; the vector grows linearly with every send.
- **Fix**: Added `AppState::push_send_history(text)` with a 1 000-entry cap. When the cap is exceeded the oldest entry is dropped and `history_index` is adjusted so the up-arrow history browser stays consistent. Replaced the three direct `send_history.push` sites with the helper. Added a unit test verifying the cap and the eviction order.
- **Decision**: FIXED.

### [Medium] Approval overlay hardcodes number keys 1–3

- **Location**: `src/tui/keys.rs::handle_approval_key`
- **Description**: The decision list (`APPROVAL_DECISIONS`) has four entries: `Allow once`, `Allow for session`, `Always allow`, `Deny`. The key handler only accepts digits `1`–`3`. Pressing `4` on a card that offers `allow-always` is silently ignored; the user must use arrow keys + Enter to reach that option.
- **Risk / trigger**: Any server-side approval card that includes the persistent `allow-always` decision. The UI advertises the option but a keyboard path to it is missing.
- **Fix**: Changed the digit arm to `c.is_ascii_digit() && c != '0'` and bound the resulting index against the live `approval.decisions.len()`. Out-of-range digits are ignored.
- **Decision**: FIXED.

### [Medium] `/btw` overlay may claim an empty run id

- **Location**: `src/tui/btw_overlay.rs::claim_run` and `::active_run_id`
- **Description**: `claim_run` stores the supplied `run_id` without checking that it is non-empty. `active_run_id` then returns `Some("")`, which is treated as a real run by `commands::reconcile_side_question` and leads to an `agent.status` RPC with an empty run id, leaving the side-question spinner active. An empty string in `claimed` could also match the rare case where another frame has no run id.
- **Risk / trigger**: A malformed or transient `AgentRunAccepted` reply with an empty `run_id`.
- **Fix**: `claim_run` now ignores empty run ids entirely, and `active_run_id` filters them out so downstream callers never see `Some("")`.
- **Decision**: FIXED.

### [Low] Chat line cache uses a sampled fingerprint

- **Location**: `src/tui/widgets/chat_area.rs::content_fingerprint`
- **Description**: The cache key validates content via `len + first 32 bytes + last 32 bytes`. Two different messages with the same byte length and matching edge bytes would collide and serve stale rendered lines until the content diverges enough to change the edges.
- **Risk / trigger**: Extremely low in practice, but possible with same-length structured output whose middle differs.
- **Suggested fix / test**: Either switch to a full hash of `content.as_bytes()` (the cache exists to avoid re-rendering, not to avoid hashing) or add a generation counter that is incremented on every message insertion/replacement. A property test that generates colliding edge strings would be valuable.
- **Decision**: REPORT ONLY — the failure is visible (wrong text on screen) and self-heals on the next edit; changing the hash strategy has a real per-frame cost that should be measured first.

## Per-perspective findings

### Security

- **Paste sanitization** now closes the ANSI/OSC/NUL injection surface at the interface boundary.
- **OSC 52 clipboard copy** (`BtwCopy`) is intentional and fire-and-forget; the code does not claim success, only that the sequence was written. This is the correct posture for an untrusted terminal.
- **No platform API calls** from this crate; `R1` is satisfied.
- **The hand-rolled base64 encoder** in `btw_overlay.rs` is exercised against RFC 4648 vectors and non-ASCII UTF-8; it is only used for the OSC 52 payload, which is base64-safe by construction.
- **A gap remains for streamed/rendered content**: assistant and system messages are rendered without stripping terminal escapes. The gateway is trusted, but defense-in-depth would sanitize at the markdown/render boundary. This is listed as a suggested test, not a fix, because it changes visible output semantics.

### Logic

- **Event-loop invariant** is preserved: one terminal/gateway event maps to one `Action`; redraw is decided by `needs_redraw` and `should_redraw_after_tick`, and gateway bursts are coalesced with `try_recv` before drawing.
- **Run/session scoping** is robust: `frame_belongs_here` checks `current_run`, the `run_sessions` FIFO (capped at 256), and `session_reconciled` for older gateways. `RunAccepted` is exempt from the guard so the mapping can be learned.
- **Streaming text deduplication** uses `turn_streamed_len` to avoid doubling `ResponseChunk` + `AgentTrace{TextEmitted{Final}}`; tests cover mixed and non-streamed turns.
- **Tool reconciliation** against `RunSummary.tool_summaries` closes the deliberately-lossy `agent_trace` mirror gap.
- **Visible-window math** in `build_visible_lines` correctly maps scroll offsets to the bottom of the transcript and includes separator rows.
- **Approval poll** is gated on an active run and only surfaces cards that belong to the current session.

### Architecture (R1–R10)

- **R4 / pure I/O**: The TUI does not persist, plan, or route business state. Knobs (`/tier`, `/mode`, `/think`, `/memory-mode`) are forwarded to `sessions.patch`; the TUI only records the result optimistically after the server accepts.
- **R5**: The status bar plus `/` → command palette is the primary entry point; overlays appear only on demand.
- **R6**: `AskUser`, tool approval, `/btw`, and transient system notices all surface gateway-driven interruptions.
- **R8 / R10**: The TUI does not classify intent or build intermediate routing; it delegates slash commands to `slash::parse_input` and then to the gateway via `send_to_agent`.
- **Command palette wiring** is complete: every local command in `slash.rs` is registered in `LOCAL_COMMAND_CATALOG` and reachable through the palette, with argument forwarding via `split_palette_input`.

### Quality

- **Production code contains no `unwrap`/`expect`**; all panicking calls are inside `#[cfg(test)]` modules.
- **Test coverage is strong**, especially around streaming deduplication, side-question frame routing, session settings restoration, and dialog/approval key handling. New tests were added for the fixes above.
- **Doc comments** consistently explain *why* a choice was made, not just *what*.

## Conclusion

`interfaces/tui` is in substantially better shape than a typical interface crate of this size. The architecture follows Aleph's redlines, the event loop is clean, and the overlay system is well-isolated. The two high findings were both unbounded/ injection risks that are now fixed at the boundary. The two medium findings were bounded UX/logic bugs that are also fixed. The one low finding is a cache-collision trade-off that should be validated with a test before changing the hashing strategy.

**What was not done / unhandled edge cases:**
- No `cargo check` / build / test was run per instructions.
- ANSI/control-character sanitization of **streamed assistant/system content** was not implemented; it is left as a defense-in-depth suggestion.
- The main transcript (`messages`) is not client-capped; memory growth there is bounded only by server-side compaction.
- `Action::CancelRun` leaves `current_run` set on RPC error; this was judged correct (the run may still be active) but could be revisited if users report a stuck spinner after a definitive "run not found" error.
- The chat-line cache fingerprint collision remains a theoretical, low-impact issue.
