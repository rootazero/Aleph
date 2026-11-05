# Phase 2 Implementation Prompt: NSH Desktop Connection Layer

**Goal:** Implement connection lifecycle + protocol handshake for NSH desktop client (Tasks 4–5 from Phase 1 plan).

**Starting Point:** `main` branch. Phase 1 complete (system.hello protocol + supervisor daemon).

**References:**
- Design: `/Volumes/TBU4/Workspace/Aleph/docs/superpowers/specs/2026-11-05-nsh-desktop-refactor-design.md` (§4–7: current state, constraints, architecture)
- Protocol: `/Volumes/TBU4/Workspace/Aleph/docs/research/NSH_PROTOCOL_V1.md` (handshake sequence, system.hello)
- ADRs: `/Volumes/TBU4/Workspace/Aleph/docs/research/NSH_ADR-001~009.md` (architecture decisions)
- Phase 1 Plan: `/Volumes/TBU4/Workspace/Aleph/docs/superpowers/plans/2026-11-05-nsh-desktop-phase1-protocol-transport.md` (context)

**Architecture (from design doc §3):**

```
desktop/nsh-shell
├── src/supervisor.rs       (Phase 1 complete)
├── src/connection.rs       (Task 4: NEW)
└── src/main.rs             (Task 5: integrate)

shared/client
└── (unchanged; reuse AlephClient for WebSocket/IPC)

shared/protocol
└── system_hello            (Phase 1 complete; Task 4 uses it)
```

**Phase 2 Tasks:**

### Task 4: Connection Layer (`desktop/nsh-shell/src/connection.rs`)

**Responsibility:** Lifecycle management for AlephClient + system.hello handshake.

**Interface:**
```rust
pub struct ShellConnection {
    client: AlephClient,
    server_version: String,
    session_key: String,
    features: Vec<Feature>,
}

pub async fn connect_to_server() -> Result<ShellConnection, String> {
    // 1. Connect via AlephClient (loopback 127.0.0.1:18790)
    // 2. Send system.hello with requested features (EventReplay, Snapshot, BinaryChannel)
    // 3. Parse response: extract server_version, session_key, negotiated features
    // 4. Return ShellConnection
}

impl ShellConnection {
    pub async fn subscribe_to_sessions(&self) -> Result<SessionSubscription, String> {
        // Call sessions.list, establish subscription for real-time updates
    }

    pub async fn close(self) -> Result<(), String> {
        // Graceful disconnect
    }
}
```

**Design constraints:**
- Use `aleph_protocol::system_hello::{HelloParams, HelloResult, Feature}` exactly as Phase 1 defined
- Loopback-only (127.0.0.1:18790); operator role implicit for local connections
- Exponential backoff (inherited from supervisor) if /ready not yet green
- No business logic; pure I/O and protocol compliance (R2/R4)

**Tests:**
- Happy path: connect → handshake → features negotiated
- Unknown feature narrowing: verify unknown features dropped (not rejected)
- Server version mismatch: fail with actionable error
- Connection drop handling: graceful close without panic

---

### Task 5: NSH Window Integration (`desktop/nsh-shell/src/main.rs` + window display)

**Responsibility:** Wire supervisor + connection into NSH window status display.

**Pseudocode flow:**
```rust
#[component]
fn App() -> impl IntoElement {
    let status = signal("Initializing...".to_string());
    let connection = signal(None::<ShellConnection>);

    spawn(async move {
        // Task 3 result
        if let Err(e) = supervisor::ensure_ready().await {
            status.set(format!("Supervisor error: {e}"));
            return;
        }

        // Task 4 result
        match connection::connect_to_server().await {
            Ok(conn) => {
                status.set(format!("Connected: {} features negotiated",
                                   conn.features.len()));
                connection.set(Some(conn));

                // Optional: start sessions subscription for live updates
                // (will be Tasks 6+ in future phases)
            }
            Err(e) => {
                status.set(format!("Connection failed: {e}"));
            }
        }
    });

    text(move || status.get())
}
```

**Design constraints:**
- No business logic in window (R2)
- Status updates only reflect connection state, not user actions
- Ready for Session UI layer (Phase 3+): placeholder for sessions.list subscription
- Graceful error reporting (no panics, all errors → status text)

---

**Execution Plan (use superpowers:subagent-driven-development):**

1. **Create task briefs & plan** (same SDD flow as Phase 1)
   - Task 4 brief: connection.rs + tests
   - Task 5 brief: main.rs integration

2. **Dispatch implementers** (fresh agents per task)
   - Task 4: connection lifecycle (mid-tier model; integration task)
   - Task 5: window integration (general-purpose; mechanical wiring)

3. **Review cycle:**
   - Spec compliance: does it match Task 4/5 requirements above?
   - Code quality: follows Rust patterns, no panics, proper error handling
   - Tests: coverage of happy path + error cases

4. **Final review:** whole-branch (main → Phase 2 branch)

---

**Environment Setup:**

```bash
# Create worktree for Phase 2
cd /Volumes/TBU4/Workspace/Aleph
git worktree add .worktrees/nsh-desktop-phase2 -b nsh-desktop-phase2

# Verify Phase 1 is present
cargo build -p aleph-nsh-shell  # should compile cleanly

# Then invoke SDD with this plan
```

**Plan File Location:** Create at `/Volumes/TBU4/Workspace/Aleph/docs/superpowers/plans/2026-11-05-nsh-desktop-phase2-connection.md`

---

**Known Unknowns (for future phases):**

- Sessions UI rendering (Phase 3+)
- Memory integration for chat history (Phase 4+)
- Plugin UI lifecycle & registration (Phase 2b+)
- Windows/Linux platform parity (parallel with core phases)

---

**Start with:**

1. Read references above (especially design doc §3–7 & protocol doc)
2. Invoke `/loop superpowers:writing-plans Phase 2 connection layer` to draft the task breakdown
3. Use `superpowers:subagent-driven-development` to execute
4. Merge to main when complete

Good luck! 🚀
