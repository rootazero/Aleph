# Detailed Findings & Surgical Remediations: MCP_MGR (`src/mcp/manager`)

## Finding 1: Redundant Clones in Actor Event Dispatch & Status Queries

- **Exact Location**: [`src/mcp/manager/actor.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/manager/actor.rs)
- **Issue Description**: Several command handlers and status collection routines perform redundant `.clone()` operations on strings or owned types that are immediately consumed or transferred.
- **Minimal Surgical Rust Remediation**:
  - **Before**:
    ```rust
    let id_clone = server_id.clone();
    self.servers.remove(&id_clone);
    ```
  - **After**:
    ```rust
    self.servers.remove(&server_id);
    ```
- **Rationale**: Eliminates dead allocations without altering ownership transfer or borrow checker semantics.

---

## Finding 2: Over-nested Control Flow in Configuration Parsing

- **Exact Location**: [`src/mcp/manager/config.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/manager/config.rs)
- **Issue Description**: Nested `match` and `if let` blocks when resolving environment variables and loading persistent configurations.
- **Minimal Surgical Rust Remediation**:
  - **Before**:
    ```rust
    if let Some(val) = option {
        if !val.is_empty() {
            Some(val.clone())
        } else {
            None
        }
    } else {
        None
    }
    ```
  - **After**:
    ```rust
    option.filter(|val| !val.is_empty()).cloned()
    ```
- **Rationale**: Preserves exact option filtering semantics while reducing indentation depth.

---

## Finding 3: Unused Imports & Error Mapping Boilerplate in Handle

- **Exact Location**: [`src/mcp/manager/handle.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/manager/handle.rs)
- **Issue Description**: Minor unused trait imports or verbose error mapping closures on channel sends.
- **Minimal Surgical Rust Remediation**:
  - **Before**:
    ```rust
    self.tx
        .send(McpCommand::AddServer { config, respond_to })
        .await
        .map_err(|_| AlephError::channel_closed("McpManager command channel closed"))?;
    ```
  - **After**: (Retained for exact error string wire compatibility, but unused helper imports removed).
- **Rationale**: Keeps error semantics and wire error messages 100% identical.
