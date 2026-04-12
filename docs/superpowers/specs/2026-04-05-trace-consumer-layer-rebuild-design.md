# Trace Consumer Layer Rebuild

## Context

A multi-phase refactoring added structured trace events to Aleph's agent loop and gateway. The backend core survived (AgentTraceEvent in protocol, LoopTraceEvent in agent_loop, StreamEvent::AgentTrace variant), but the consumer/presentation layer was lost before commit. This spec covers rebuilding those missing pieces.

## Existing Infrastructure

| Component | Status | Location |
|-----------|--------|----------|
| AgentTraceEvent enum | Exists | `shared/protocol/src/events.rs:232` |
| AgentTraceEvent::kind() | Exists | `shared/protocol/src/events.rs:274` |
| LoopTraceEvent model | Exists | `src/agent_loop/trace.rs` (265 lines) |
| StreamEvent::AgentTrace variant | Exists | `shared/protocol/src/events.rs` |
| ReplayService (empty stub) | Exists | `src/resilience/database/replay.rs` (16 lines) |
| Gateway trace_replay handler | Exists | `src/gateway/handlers/trace_replay.rs` |
| CLI agent_trace consumption | Exists | `interfaces/cli/src/commands/ask.rs` |
| TUI agent_trace consumption | Exists | `interfaces/tui/src/tui/app.rs` (references missing functions) |

## Missing Components

### Layer 1: Protocol Shared Presentation

**File: `shared/protocol/src/trace_presentation.rs`**

Provides shared trace event formatting that CLI, TUI, and webchat all consume. Single source of truth for trace text rendering.

Types:
- `AgentTracePresentationPreset` — enum with `CliCompact`, `TuiDebug`, `PanelTrace`, each carrying appropriate truncation limits
- `AgentTracePresentationOptions` — truncation config (content_limit, tool_input_limit, tool_output_limit)
- `AgentTracePresentationLabels` — English text labels for trace events
- `AgentTracePresentation` — output struct: kind, status, content, duration_ms

Functions:
- `present_agent_trace_event(event, options, labels) -> AgentTracePresentation` — main projection
- `present_agent_trace_event_with_preset(event, preset) -> AgentTracePresentation` — convenience with English labels
- `present_agent_trace_event_with_labels_and_preset(event, labels, preset) -> AgentTracePresentation` — custom labels + preset options
- `summarize_tool_input(params: &Value, limit: usize) -> String` — key=value summary of tool parameters
- `summarize_tool_output(output: &str, limit: usize) -> String` — truncated tool output
- `summarize_tool_result(result: &AgentTraceToolResult, limit: usize) -> String` — result wrapper

Export from `shared/protocol/src/lib.rs`.

**File: `shared/protocol/src/trace_replay.rs`**

Replay DTOs for querying persisted traces.

Types:
- `AgentTraceReplayEntry` — { step: u64, event: AgentTraceEvent }
- `AgentTraceReplay` — { task_id, started_at, status, total_events, events: Vec<AgentTraceReplayEntry> }
- `AgentTraceReplayListItem` — { task_id, started_at, status, event_count } (for list view)

Export from `shared/protocol/src/lib.rs`.

### Layer 2: Gateway Replay Handlers

**File: `src/gateway/handlers/trace_replay.rs`** (already exists, needs real implementation)

Handlers:
- `trace.list` — query recent tasks with trace metadata, returns Vec<AgentTraceReplayListItem>
- `trace.get` — query full trace for a task_id, returns AgentTraceReplay

Both query through ReplayService using the existing StateDatabase task_traces table.

**File: `src/resilience/database/replay.rs`** (already exists, needs methods)

Methods:
- `list_recent_traces(limit: usize) -> Vec<AgentTraceReplayListItem>` — join agent_tasks + task_traces count
- `get_trace(task_id: &str) -> Option<AgentTraceReplay>` — full trace with events

### Layer 3: Frontend Consumers

**File: `interfaces/webchat/src/api/trace.rs`**

Leptos API client calling trace.list / trace.get via gateway JSON-RPC.

**File: `interfaces/webchat/src/views/agent_trace_model.rs`**

Pure model layer:
- `TraceLabels` — localization-ready labels, default from PanelTrace preset
- `trace_node_from_agent_trace_event(event, labels) -> TraceNode` — structured projection
- `trace_nodes_from_replay(replay, labels) -> Vec<TraceNode>` — batch replay conversion
- `LegacyTraceNodeEvent` + `trace_node_from_legacy_event(event, labels) -> Option<TraceNode>` — gateway legacy adapter

**File: `interfaces/webchat/src/views/agent_trace.rs`**

Page component with Live/Replay dual mode:
- Live mode: subscribes to run.agent_trace events, legacy fallback for run.tool_start/tool_end
- Replay mode: loads from trace.get API
- Shared rendering using model layer

**File: `interfaces/cli/src/commands/trace_cmd.rs`**

CLI subcommand:
- `aleph trace list` — show recent traces (CliCompact preset)
- `aleph trace show <task_id>` — show full trace events

## Design Decisions

1. **No over-splitting**: Keep webchat trace in 3 files (api, model, view), not 7+ sub-modules
2. **Shared presentation is the truth**: All three frontends consume the same projection functions
3. **Legacy fallback in webchat only**: CLI/TUI only consume agent_trace events; webchat has legacy support for older run.tool_start/tool_end
4. **Presets over raw options**: Consumers select a named preset, not raw truncation numbers

## Build Order

1. `trace_presentation.rs` + `trace_replay.rs` (protocol layer, zero deps)
2. Fix TUI compilation (it already imports these missing functions)
3. `replay.rs` methods + `trace_replay.rs` handler (backend)
4. `trace_cmd.rs` (CLI, simple consumer)
5. Webchat files (api + model + view)
6. Verify: `cargo check -p aleph-protocol -p aleph-cli -p aleph-tui -p aleph-panel`

## Testing Strategy

- Protocol layer: unit tests for presentation formatting, serialization roundtrip
- Replay service: focused tests with in-memory SQLite
- CLI: command parsing tests
- Webchat model: pure function tests for trace_node_from_agent_trace_event
