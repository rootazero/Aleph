# Occam's Razor Review Brief: Looping, Markdown, and MCP

## 1. Scope & Invariants

This review targets `src/looping`, `src/markdown`, and `src/mcp` under Occam's Razor doctrine:
- Simplicity is the removal of unnecessary concepts, not merely line count reduction.
- Every abstraction, clone, intermediate allocation, and indirection must earn its existence.

### Subsystem Invariants

#### Looping (`src/looping`)
1. **Loop State Transitions & Invariants**:
   - `LoopState` variants (`Active`, `Paused`, `Stopped`, `Exhausted`, etc.) must preserve valid state machine transitions.
   - Iteration caps, budget enforcement, and monotonic tick progression must remain exact.
2. **Pursuit Timing & Deadlines**:
   - `should_fire`, `is_final_deadline_tick`, `cadence_hint`, and `tick_delay_ms` must preserve arithmetic precision and boundary conditions (e.g. `now_ms = 0` handling, deadline expiration clamping).
   - Prompt generation (`tick_prompt`) quota and warning clauses must maintain message clarity and priority rules.
3. **Actor & Resource Concurrency**:
   - Task cancellation, join handles, channel receivers, and loop termination signals must never deadlock or leak tasks.

#### Markdown (`src/markdown`)
1. **Fences Extraction & Syntax**:
   - Fenced code block detection (3+ backticks or tildes, language tags, info strings) must handle leading spaces, unclosed fences, and escaped characters faithfully.
   - Streaming/chunked extraction behavior must remain robust against split boundaries.
2. **Allocation Efficiency**:
   - Prefer slice borrowing (`&str`) and zero-copy references over owned `String` allocations where appropriate.

#### MCP (`src/mcp`)
1. **JSON-RPC & Protocol Specification**:
   - Adhere strictly to Model Context Protocol (MCP) JSON-RPC 2.0 specifications.
   - Request IDs, error code formatting, tool descriptors, prompt arguments, and resource URIs must preserve wire schema compatibility.
2. **Transport Integrity (Stdio, HTTP, SSE, MRTR)**:
   - Stdio process lifecycle (spawn, stdin pipe flush, stdout line buffering, stderr capture, kill on drop) must remain leak-free and handle EOF cleanly.
   - SSE event stream framing (`event:`, `data:`, `id:`, double newlines) and reconnection tokens must remain compliant with SSE specs.
   - MRTR transport headers, chunk streaming, and cache invalidation must preserve freshness invariants.
3. **Security, Auth & Sanitization**:
   - Secret redaction (`redact.rs`): sensitive keys, passwords, and tokens must never leak to logs or tool output.
   - Auth tokens, OAuth state callback matching, and persistent credentials must prevent replay attacks and TOCTOU bugs.
   - Tool argument sanitization and preflight checks must fail closed on hostile payloads.
4. **Manager Actor Lifecycle & Concurrency**:
   - Actor loop message dispatch, handle cloning, graceful shutdown cascades, and secret resolution must preserve lock orders and avoid lock-inversion across actor bounds.

---

## 2. Methodology & Model Tiering

### Jev Tier (Classification, Filtering, Routing)
The **Jev tier** (fast pattern matching, classification, taxonomy tagging):
1. **Scan**: Scan assigned batch files for Occam's Razor anti-patterns.
2. **Classify**: Categorize candidate issues into:
   - `DEAD-CODE` / `ACCIDENTAL-COMPLEXITY`
   - `REDUNDANT-ALLOCATION` (unnecessary `.clone()`, `to_string()`, redundant vectors)
   - `CONTROL-FLOW` / `INDIRECTION` (over-nested loops, unneeded wrapper layers)
   - `PANIC-RISK` / `DEFENSIVE-CHECK` (unwrap on untrusted data, missing bounds check)
   - `CORRECTNESS` / `INVARIANT-TIGHTENING` (TOCTOU, silent error discard, off-by-one)
3. **Route**: Tag each finding with:
   - `FIX-NOW`: Direct safe simplification or bug fix.
   - `FIX-CAREFULLY`: Nuanced invariant or allocation tightening requiring careful semantics check.
   - `DEFER`: Architectural or wire-breaking changes.
   - `REJECT`: Deliberate design patterns or required test scaffolding.
4. Output: Write `classification/<BATCH_ID>.class.md`.

### Deep Reasoning Tier (minimax-M3 / Invariant Analysis & Code Repair)
The **Deep Reasoning tier**:
1. Verify semantic preservation for all items routed for fix.
2. Author surgical patches directly in worktree `/home/zou/data/workspace/Aleph-wt-looping-markdown-mcp`.
3. Apply Occam's Razor decision hierarchy:
   - Borrowed references (`&str`, `&[T]`) instead of owned copies where public API allows.
   - Idiomatic Rust: `?`, `let ... else`, `if let`, early return.
   - Remove unused private helpers and redundant clones.
4. Document findings and verification in `findings/<BATCH_ID>.md`.
5. Direct commit per batch without PR and without interim cargo check.
