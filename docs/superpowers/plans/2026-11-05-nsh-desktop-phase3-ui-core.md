# NSH Desktop Phase 3: Core UI Implementation

**Status:** Ready for implementation  
**Prerequisites:** Phase 2 complete (connection layer integrated)  
**Target:** Implement core UI components for NSH Desktop  
**Estimated effort:** 3-5 days

---

## Context

### What Phase 2 Delivered

Phase 2 (commits e8696f390..ad5b52472) implemented the connection layer:

- **Task 4**: Connection loop with reconnection, timeout, protocol validation (18 tests passing)
- **Task 5**: Integration into NSH window (`main.rs` now shows live connection status)
- **Current behavior**: Window displays "Connecting..." → "Connected: server vX, N sessions" → "Error: ..."

**Current HEAD**: edb23b67f (includes architecture docs)

### What Phase 3 Will Deliver

Replace the placeholder text display with a functional UI:

1. **Input Field**: Text input for user prompts
2. **Message List**: Scrollable conversation history
3. **Message Rendering**: User messages + Assistant responses
4. **Tool Call Cards**: Visual representation of tool invocations
5. **Streaming Support**: Real-time token display during generation

---

## Architecture Context

### ADR-001: Mixed Runtime (Updated with Web Target)

**Location:** `docs/research/NSH_ADR-001~009.md`

**Key decisions:**
- Rust Native Components for high-performance core interactions
- TSX for page composition and business screens
- Each Native Component must declare Web fallback strategy
- Web is a first-class target, not a Desktop addon

**For Phase 3:**
- Input field, message list, streaming text: **Rust Native Components** (high-frequency, complex lifecycle)
- Message container, layout: **TSX** (composition)
- Tool call cards: **TSX** (structured data display)

### PROTOCOL_V1

**Location:** `docs/research/NSH_PROTOCOL_V1.md`

**Relevant sections:**
- § 4.2: `system.hello` handshake (already implemented in Phase 2)
- § 6.2: `session.*` methods (create, list, get, snapshot)
- § 6.3: `message.*` methods (send, get, list)
- § 6.4: `agent.*` methods (run, get_task, cancel)
- § 8: Event Envelope (event_id, scope, sequence, name, payload)
- § 9: Snapshot + Replay (consistency, no-loss subscription)

**Phase 3 will use:**
- `message.send` to submit user input
- `message.list` to load conversation history
- Event subscription for streaming tokens and tool calls

### NSH Path Dependency

**Locked version:** `nsh = { git = "https://github.com/yours/nsh.git", rev = "2ab3b6bc27" }`

**Phase 1 verification:**
- NSH `signal`, `spawn`, `text`, `component` macros work
- Event loop integration tested in Phase 1 spike

**Phase 3 components will use:**
- `signal` for reactive state
- `spawn` for async updates
- `text`, `input`, `scroll_view` for UI primitives
- `nsh_core::prelude::*` imports

---

## Design Reference

### Phase 0 Spike Results

**Location:** `docs/research/NSH_DESIGN_DOC.md` § 4.3

**Key findings:**
- Signal-based reactivity works for streaming updates
- Async spawn bridges Tokio and NSH event loop
- Input handling requires careful lifecycle management
- Scroll performance acceptable for message lists up to 1000 items

### UI Component Hierarchy

```
App (main.rs)
├── ConnectionStatus (existing, shows "Connecting..." / "Connected")
├── SessionView (new)
│   ├── MessageList (Rust Native Component)
│   │   ├── UserMessage (TSX)
│   │   ├── AssistantMessage (TSX)
│   │   └── ToolCallCard (TSX)
│   └── InputField (Rust Native Component)
└── Sidebar (future: Phase 4)
```

### State Management

**Connection state** (existing in Phase 2):
```rust
enum ShellStatus {
    Connecting,
    Connected { server_version: String, session_count: usize },
    Error(String),
}
```

**Session state** (new in Phase 3):
```rust
struct SessionState {
    session_id: Option<String>,
    messages: Vec<Message>,
    input_text: String,
    is_generating: bool,
}
```

**Message protocol** (from PROTOCOL_V1):
```json
{
  "message_id": "msg-001",
  "role": "user" | "assistant",
  "content": "...",
  "timestamp": "2026-11-05T10:00:00Z",
  "tool_calls": [...]
}
```

---

## Implementation Tasks

### Task 6: Session State Management

**File:** `desktop/nsh-shell/src/session.rs` (new)

**Responsibilities:**
1. Hold current session ID
2. Maintain message list
3. Subscribe to message events
4. Handle snapshot + replay for reconnection
5. Provide reactive signals for UI binding

**Interface:**
```rust
pub struct SessionManager {
    client: Arc<AlephClient>,
    session_id: Option<String>,
    messages: Signal<Vec<Message>>,
    is_generating: Signal<bool>,
}

impl SessionManager {
    pub async fn create_session(&mut self) -> Result<String>;
    pub async fn load_messages(&mut self) -> Result<()>;
    pub async fn send_message(&mut self, content: String) -> Result<()>;
    pub fn subscribe_events(&mut self);
}
```

**Tests:**
- Create session via RPC
- Load message history
- Subscribe to events
- Handle reconnection (snapshot + replay)

### Task 7: Input Field Component

**File:** `desktop/nsh-shell/src/components/input.rs` (new)

**Requirements:**
1. Multi-line text input
2. Submit on Cmd+Enter (macOS) / Ctrl+Enter (Windows/Linux)
3. Disabled during generation
4. Clear after send
5. Auto-focus after send completes

**Pattern from Phase 1 spike:**
```rust
#[component]
fn InputField(on_submit: impl Fn(String) + 'static) -> impl IntoElement {
    let text = signal(String::new());
    
    input(move || text.get())
        .on_change(move |new_text| text.set(new_text))
        .on_submit(move || {
            on_submit(text.get());
            text.set(String::new());
        })
}
```

**Tests:**
- Text input updates signal
- Submit callback fires
- Input clears after submit
- Disabled state blocks submit

### Task 8: Message List Component

**File:** `desktop/nsh-shell/src/components/message_list.rs` (new)

**Requirements:**
1. Scrollable container
2. Render user + assistant messages
3. Auto-scroll to bottom on new message
4. Virtual scrolling for 1000+ messages (future optimization)

**Pattern:**
```rust
#[component]
fn MessageList(messages: Signal<Vec<Message>>) -> impl IntoElement {
    scroll_view(move || {
        messages.get().iter().map(|msg| {
            match msg.role {
                Role::User => user_message(msg),
                Role::Assistant => assistant_message(msg),
            }
        }).collect()
    })
}
```

**Tests:**
- Renders empty list
- Renders multiple messages
- Auto-scrolls on append
- Handles message updates (streaming)

### Task 9: Streaming Token Display

**File:** `desktop/nsh-shell/src/streaming.rs` (new)

**Requirements:**
1. Listen for `message.delta` events
2. Accumulate tokens into message buffer
3. Update UI signal on each delta
4. Finalize message on `message.complete` event

**Event protocol** (from PROTOCOL_V1 § 8):
```json
{
  "type": "event",
  "event": {
    "name": "message.delta",
    "payload": {
      "message_id": "msg-001",
      "delta": "Hello",
      "index": 0
    }
  }
}
```

**Implementation:**
```rust
pub struct StreamingBuffer {
    current_message: Signal<String>,
    deltas: Vec<String>,
}

impl StreamingBuffer {
    pub fn append_delta(&mut self, delta: String) {
        self.deltas.push(delta);
        self.current_message.set(self.deltas.join(""));
    }
    
    pub fn finalize(&mut self) -> String {
        self.deltas.join("")
    }
}
```

**Tests:**
- Accumulate multiple deltas
- Update signal on each append
- Finalize produces complete text
- Handle out-of-order deltas (sequence check)

### Task 10: Integration into App

**File:** `desktop/nsh-shell/src/main.rs` (modify)

**Changes:**
1. Replace placeholder text with `SessionView`
2. Wire `SessionManager` to connection state
3. Handle connection → session lifecycle
4. Error boundary for RPC failures

**Pattern:**
```rust
#[component]
fn App() -> impl IntoElement {
    let connection_status = signal(ShellStatus::Connecting);
    let session_manager = signal(None::<SessionManager>);
    
    // Existing connection loop from Phase 2
    spawn_connection_loop(/* ... */);
    
    // New: Create session when connected
    spawn(async move {
        if let ShellStatus::Connected { .. } = connection_status.get() {
            let manager = SessionManager::new(client).await.ok();
            session_manager.set(manager);
        }
    });
    
    // Render
    container(move || {
        if let Some(manager) = session_manager.get() {
            session_view(manager)
        } else {
            text(move || format!("Status: {:?}", connection_status.get()))
        }
    })
}
```

**Tests:**
- Session created on connection
- Input submits message
- Message appears in list
- Streaming updates display

---

## Verification Checklist

### Unit Tests

```bash
cargo test -p aleph-nsh-shell
```

Expected: All tests pass (18 existing + new component tests)

### Integration Test

**Scenario 1: Happy path**
1. Start server: `cargo run -p aleph-server -- start`
2. Start NSH: `cargo run -p aleph-nsh-shell`
3. Type message in input field
4. Press Cmd+Enter
5. Verify: Message appears in list, streaming response displays

**Scenario 2: Reconnection**
1. Connect to server
2. Kill server process
3. Restart server
4. Verify: UI reconnects, conversation history restored

**Scenario 3: Error handling**
1. Server returns RPC error
2. Verify: Error message displays, input re-enables

### Performance

- Message list scroll: smooth at 100 messages
- Streaming latency: < 50ms per token
- Input responsiveness: no lag during generation

---

## Dependencies

### Rust Crates

```toml
[dependencies]
nsh = { git = "https://github.com/yours/nsh.git", rev = "2ab3b6bc27" }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

### Server RPC Methods (must exist)

- `session.create` → `{ session_id: string }`
- `message.send` → `{ message_id: string }`
- `message.list` → `{ messages: [...] }`
- `events.subscribe` → `{ subscription_id: string }`

**Verification:**
```bash
# Check server implements required RPCs
rg "session.create|message.send|message.list" server/src/rpc/
```

---

## Risk Mitigation

### Risk 1: NSH API mismatch

**Mitigation:** Verify NSH components at rev `2ab3b6bc27` before implementation:
```bash
cd /tmp && git clone https://github.com/yours/nsh.git && cd nsh
git checkout 2ab3b6bc27
rg "pub fn input|pub fn scroll_view" src/
```

### Risk 2: Server RPC not implemented

**Mitigation:** Implement server stubs first, or mock for UI development:
```rust
#[cfg(test)]
mod mock {
    pub struct MockClient;
    impl AlephClient for MockClient {
        async fn send_message(&self, _: String) -> Result<String> {
            Ok("msg-001".into())
        }
    }
}
```

### Risk 3: Streaming performance

**Mitigation:** 
- Batch small deltas (< 10 chars) into 50ms windows
- Use virtual scrolling if message count > 1000
- Profile with `cargo flamegraph` if frame drops occur

---

## Rollback Plan

If Phase 3 blocks (NSH API issues, server RPC unavailable):

1. Keep Phase 2 connection layer (already working)
2. Revert UI changes: `git reset --hard ad5b52472`
3. Implement server RPC methods first
4. Resume Phase 3 when dependencies ready

---

## References

### Essential Documents

1. **ADR-001~009**: `docs/research/NSH_ADR-001~009.md`  
   Architecture decisions, Mixed Runtime, Web Target
   
2. **PROTOCOL_V1**: `docs/research/NSH_PROTOCOL_V1.md`  
   RPC methods, Event Envelope, Snapshot/Replay
   
3. **Phase 2 Plan**: `docs/superpowers/plans/2026-11-05-nsh-desktop-phase2-connection.md`  
   Connection layer implementation (predecessor)
   
4. **Phase 1 Results**: `docs/research/NSH_DESIGN_DOC.md` § 4.3  
   NSH spike findings, signal + spawn patterns

### Code References

1. **Phase 2 connection layer**: `desktop/nsh-shell/src/connection.rs`  
   Pattern for async RPC + event subscription
   
2. **Phase 1 spike**: `desktop/nsh-spike/src/main.rs`  
   Signal + spawn example, NSH component usage
   
3. **Server RPC handlers**: `server/src/rpc/` (if exists)  
   Protocol implementation examples

### External Resources

1. **NSH documentation**: Check repo at rev `2ab3b6bc27`
2. **JSON-RPC 2.0 spec**: https://www.jsonrpc.org/specification
3. **Tokio async patterns**: https://tokio.rs/tokio/tutorial

---

## Success Criteria

Phase 3 is complete when:

- [ ] `cargo test -p aleph-nsh-shell` passes (all tests)
- [ ] `cargo build -p aleph-nsh-shell` succeeds (no warnings)
- [ ] Manual QA: Can type message, submit, see response
- [ ] Manual QA: Streaming tokens display in real-time
- [ ] Manual QA: Reconnection preserves conversation
- [ ] Code review: Native Components have Web fallback documented
- [ ] Commit message follows convention: `feat(nsh): implement Phase 3 core UI`

---

## Implementation Notes

### Commit Strategy

Follow Phase 2 pattern:
1. Initial implementation commit (tests passing)
2. Fix rounds if issues found in review
3. Architecture docs if needed

### Code Style

- **Rust formatting**: `cargo fmt` before commit
- **Clippy**: `cargo clippy -- -D warnings` must pass
- **Test coverage**: Aim for 80%+ on new code
- **Documentation**: Each component has module-level doc comment

### Collaboration

If blocked:
1. Check Phase 2 implementation for similar patterns
2. Review PROTOCOL_V1 for RPC/event specs
3. Test with mock client if server unavailable
4. Ask user for clarification on ambiguous requirements

