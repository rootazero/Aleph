# Proactive AI Architecture Plan for Aleph

**Date:** 2026-02-04
**Status:** Proposed (Refined)
**Reference Project:** OpenClaw (Analysis Completed)

## 1. Objective

To evolve `Aether` from a reactive, user-prompted CLI tool into a proactive, autonomous assistant ("JARVIS-like"). It must run in the background, perceive the user's environment, maintain a "World Model," and autonomously initiate helpful actions without explicit commands, while respecting system resources and user privacy.

## 2. Reference Analysis: OpenClaw

`OpenClaw` provides a practical blueprint for OS integration:

1.  **Gateway Service (`src/daemon`):**
    - Abstracts OS-specific service managers (`launchd` for macOS, `systemd` for Linux, Task Scheduler for Windows).
    - Manages lifecycle (install, uninstall, start, stop) via a unified interface.
2.  **Hook System (`src/hooks`):**
    - A central registry (`internal-hooks.ts`) for event subscriptions (`command`, `session`, `agent`).
    - Decoupled "Watchers" (e.g., `gmail-watcher`) that can run as separate child processes (using helper binaries like `gog`) to ensure stability.
3.  **Agent Integration:**
    - Agents emit lifecycle events (e.g., `agent:bootstrap`), allowing the hook system to inject context or modify behavior dynamically.

## 3. Gap Analysis & Strategy

| Feature | OpenClaw Approach | Aleph (Current) | Aleph Strategy (Target) |
| :--- | :--- | :--- | :--- |
| **Persistence** | `GatewayService` (Node.js) | CLI (Ephemeral) | **`DaemonManager` (Rust)**: Cross-platform service abstraction. |
| **Sensation** | Specialized child processes (`gog`) | None | **`PerceptionLayer`**: Native Rust traits for lightweight watchers; Child processes for heavy/risky ones. |
| **Cognition** | Logic in Hooks | User Prompt | **`WorldModel`**: A shared state representing the user's current context (e.g., "Coding", "Meeting"). |
| **Action** | Trigger -> Agent | Direct Execution | **`Dispatcher`**: An autonomous loop that evaluates `WorldModel` changes against policies to trigger Agents. |

## 4. Architectural Proposal

### 4.1. Module 1: Daemon Manager (`core/src/daemon`)

**Purpose:** Ensure Aleph runs continuously and survives reboots.

**Components:**
1.  **`ServiceManager` Trait:**
    - Methods: `install()`, `uninstall()`, `start()`, `stop()`, `status()`.
    - Implementations: `LaunchdService` (macOS), `SystemdService` (Linux), `WindowsService`.
2.  **IPC Server:**
    - The daemon must expose a local socket (Unix Domain Socket / Named Pipe) to receive commands from the CLI (`aether start`, `aether status`) and send notifications to the UI.
3.  **Resource Governor:**
    - Monitors own CPU/RAM usage. Pauses heavy "proactive" tasks if system load is high or running on battery (critical for "invisible" assistant).

### 4.2. Module 2: Perception Layer (`core/src/perception`)

**Purpose:** The "Eyes and Ears". Collect raw data and convert it into `Events`.

**Architecture:**
- **`Watcher` Trait:**
    ```rust
    #[async_trait]
    trait Watcher {
        fn id(&self) -> &str;
        async fn run(&self, event_bus: &EventBus); // Pushes events to bus
    }
    ```
- **Planned Watchers:**
    1.  **`FSEventWatcher`:** Monitors key directories (Downloads, Desktop, Git repos) for changes.
    2.  **`ProcessWatcher`:** Detects active application focus (VS Code, Browser, Zoom).
    3.  **`TimeWatcher`:** Advanced scheduler (Cron + "Time until X").
    4.  **`SystemStateWatcher`:** Battery level, Network status, Idle time.

### 4.3. Module 3: World Model (`core/src/context/world_model.rs`)

**Purpose:** The "Memory". Aggregates raw events into high-level state.

**Function:**
- Subscribes to `EventBus`.
- Maintains a `State` struct:
    ```rust
    struct WorldState {
        user_activity: Activity, // e.g., Coding, Browsing, Idle
        system_load: LoadLevel,
        pending_tasks: Vec<Task>,
        recent_events: CircularBuffer<Event>,
    }
    ```
- **Inference:** Uses lightweight logic (or small local LLM) to deduce context.
    - *Example:* `Process(VSCode)` + `FileChange(.rs)` -> `Activity::Coding(Rust)`.

### 4.4. Module 4: Dispatcher (`core/src/dispatcher`)

**Purpose:** The "Brain". Decides *when* to act.

**Workflow:**
1.  **Trigger:** `WorldModel` updates or `TimeWatcher` ticks.
2.  **Policy Check:**
    - "Is this action allowed proactively?" (User preferences).
    - "Is it safe?" (Resource Governor check).
    - "Confidence Score?" (If low, ask user: "Shall I...?").
3.  **Execution:** Spawns an ephemeral `Agent` session with the specific context.

## 5. Implementation Roadmap

### Phase 1: The Backbone (Daemon)
1.  Implement `ServiceManager` for macOS (`launchd`).
2.  Create the `aether daemon` CLI subcommand.
3.  Set up the IPC channel between CLI and Daemon.

### Phase 2: Senses (Watchers)
1.  Implement `EventBus` (broadcast channel).
2.  Build `ProcessWatcher` (using `sysinfo`) and `FSEventWatcher` (using `notify`).
3.  Wire watchers to print events to the daemon log (verification).

### Phase 3: The Brain (Dispatcher & Model)
1.  Implement `WorldModel` struct and state transitions.
2.  Create a simple `Dispatcher` with hardcoded rules (e.g., "If `Downloads` folder > 1GB, trigger Cleanup Agent").
3.  Connect `Dispatcher` to the existing Agent execution engine.

### Phase 4: Integration (JARVIS Mode)
1.  Implement "Active" vs "Passive" modes.
2.  Add User Feedback loop (System Notifications for actions taken).
3.  **Privacy Control:** A TUI dashboard showing exactly what Aleph is watching and doing.

## 6. Key Principles for "JARVIS" Quality

- **Invisibility:** Never interrupt the user's flow unless critical.
- **Frugality:** Zero impact on foreground app performance.
- **Transparency:** "I archived 5 old files from Desktop" (Notification) -> Click to Undo.
- **Safety:** Proactive actions are *sandboxed* and reversible where possible.