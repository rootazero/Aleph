# WebChat Rust Rewrite Design

## Goal

Rewrite the TypeScript/React WebChat (~993 LOC) as a Leptos route integrated into the existing Panel WASM application, achieving a pure Rust product with zero JS/npm dependencies.

## Architecture Decision

**Integrate into Panel** rather than maintaining a separate application:

- WebChat becomes `/chat` route in Panel's Leptos Router
- Shares Panel's WebSocket connection, theme system, and Tailwind config
- Single WASM build, single deployment artifact
- Eliminates separate Node.js/Vite build pipeline

```
Panel (Leptos/WASM)
├── /dashboard        — existing dashboard
├── /chat             — chat interface (this design)
├── /settings         — existing settings
├── /agents           — existing agent management
     ↓
  Shared Gateway WebSocket (JSON-RPC 2.0)
```

## Technical Choices

### Markdown Rendering: Pure Rust

- **Parser**: `pulldown-cmark` — mature Rust Markdown parser, GFM support
- **Syntax Highlighting**: `syntect` — 200+ languages, TextMate grammar compatible
- **Rendering**: Parse Markdown AST → Leptos `view!` nodes (type-safe, XSS-free)
- No JS interop needed

### Streaming: Follow System Mode

Core's `GatewayEventEmitter` handles mode switching server-side:

- `behavior.output_mode: "typewriter"` → chunks emitted incrementally
- `behavior.output_mode: "instant"` → chunks buffered, single emission on final

Client simply renders whatever arrives via `stream.response_chunk` events. No client-side mode logic needed.

### WebSocket: Reuse Panel Connection

Chat page subscribes to Panel's existing Gateway WebSocket manager. No separate connection.

Events consumed:
- `stream.response_chunk` — message content (incremental or complete)
- `stream.run_accepted` — agent started processing
- `stream.run_complete` — response finished
- `stream.run_error` — error notification
- `stream.tool_start/update/end` — tool execution status
- `stream.reasoning` — thinking updates

RPC methods called:
- `agent.run` — send message
- `sessions.list` — fetch session list
- `sessions.history` — load message history
- `commands.list` — fetch slash commands for palette
- `behavior.get` — query current output mode

## Feature List (1:1 migration from TS WebChat)

1. **Message bubbles** — user/assistant/system role styling
2. **Markdown rendering** — GFM with code block syntax highlighting + copy button
3. **Streaming output** — follows system typewriter/instant mode
4. **Session sidebar** — session list, new session button, connection status
5. **Slash command palette** — `/` trigger, keyboard navigation (↑↓ + Enter)
6. **Dark mode** — follows Panel global theme
7. **Connection status** — connected/connecting/disconnected indicator
8. **Responsive layout** — mobile sidebar toggle, adaptive message widths

## Dependencies

```toml
# New additions to apps/panel/Cargo.toml
pulldown-cmark = "0.12"
syntect = { version = "5", default-features = false, features = ["default-fancy"] }
```

## Code Reuse

| Source | What to reuse |
|--------|--------------|
| `shared/ui_logic` | Shared type definitions |
| `shared/protocol` | JSON-RPC protocol types |
| Panel WebSocket layer | Connection management, reconnection, event dispatch |
| Panel Tailwind config | Theme, color palette, dark mode classes |
| Panel component patterns | Signal-based state, view macros |

## Estimated Scope

~2000 LOC across:

| Component | LOC | Description |
|-----------|-----|-------------|
| Chat page | ~500 | Main layout, message area, scroll management |
| Message bubble | ~600 | Markdown parsing, syntax highlighting, copy button |
| Command palette | ~300 | Slash command list, keyboard navigation, filtering |
| Sidebar | ~300 | Session list, status indicator, new chat |
| Route integration | ~300 | Router config, event subscription, state wiring |

## What Gets Deleted

After integration is complete:
- `apps/webchat/` — entire TypeScript application
- Remove webchat from any build scripts or CI references
