# Testing Guide: Client Architecture Refactoring

This guide provides comprehensive testing procedures for the Client Architecture Refactoring (Phase 1-3), which migrated all clients from Fat Client (embedded alephcore) to Thin Client (Gateway communication via SDK).

## Overview

The refactoring transformed the architecture from:
```
Client → Embedded AlephCore → AI Providers
```

To:
```
Client → aleph-client-sdk → WebSocket → Aleph Gateway → AlephCore → AI Providers
```

## Prerequisites

### 1. Gateway Server

The Gateway must be running before testing any client:

```bash
# Start Gateway in foreground (for debugging)
cargo run -p alephcore --features gateway --bin aleph-gateway -- start --log-level debug

# Or start as daemon (background)
cargo run -p alephcore --features gateway --bin aleph-gateway -- start --daemon

# Check status
cargo run -p alephcore --features gateway --bin aleph-gateway -- status
```

**Default Gateway Address**: `ws://127.0.0.1:18789`

### 2. Configuration

Ensure you have a valid Aleph configuration with at least one AI provider configured:

```bash
# Check config location
ls ~/.aleph/config.toml

# Or use custom config
cargo run -p alephcore --features gateway --bin aleph-gateway -- start -c /path/to/config.toml
```

## Testing Checklist

### Phase 1: Directory Structure ✅

**Verification**:
```bash
# Verify new structure
ls -la apps/
# Should show: cli/, macos/, desktop/

# Verify old structure is gone
ls -la platforms/
# Should show: (empty or not exist)
```

**Expected Result**: All client code moved from `platforms/` to `apps/`

---

### Phase 2: SDK Implementation ✅

#### 2.1 SDK Build Test

```bash
# Build SDK
cargo build -p aleph-client-sdk

# Run SDK tests
cargo test -p aleph-client-sdk

# Check features
cargo build -p aleph-client-sdk --features transport,rpc,client,local-executor
```

**Expected Result**: All builds and tests pass

#### 2.2 CLI Client Test

```bash
# Build CLI
cargo build -p aleph-cli

# Test CLI help
cargo run -p aleph-cli -- --help

# Test CLI connection (requires Gateway running)
cargo run -p aleph-cli -- "Hello, test the connection"

# Test CLI with specific provider
cargo run -p aleph-cli -- --provider claude "Explain quantum computing"
```

**Expected Behavior**:
- CLI connects to Gateway via WebSocket
- Authentication succeeds
- Streaming responses display correctly
- Tool calls (if any) execute properly

**Verification Points**:
- [ ] CLI connects without errors
- [ ] Authentication token is saved to config
- [ ] Streaming responses work
- [ ] Tool execution works (try: "What files are in the current directory?")
- [ ] Error handling works (try with Gateway stopped)

---

### Phase 3: Tauri Desktop Client ✅

#### 3.1 Build Test

```bash
cd apps/desktop

# Install frontend dependencies
pnpm install

# Build frontend
pnpm build

# Build Tauri backend
cd src-tauri && cargo build
```

**Expected Result**: All builds succeed with only warnings (no errors)

#### 3.2 Development Mode Test

```bash
cd apps/desktop

# Run in dev mode (opens GUI)
pnpm tauri dev
```

**Manual Testing Checklist**:

**Connection & Authentication**:
- [ ] App starts without crashes
- [ ] Gateway connection established automatically
- [ ] Authentication succeeds (check logs)
- [ ] Connection status indicator shows "connected"

**Basic Functionality**:
- [ ] Input field accepts text
- [ ] Send button triggers request
- [ ] Streaming responses display in real-time
- [ ] Response formatting (markdown, code blocks) works
- [ ] Multiple messages in conversation work

**RPC Proxy Commands** (16 total):

**Core Commands**:
- [ ] `process_input` - Send message and receive response
- [ ] `stop_generation` - Cancel ongoing generation
- [ ] `get_topics` - List conversation topics
- [ ] `get_topic_messages` - Load topic history
- [ ] `delete_topic` - Delete a topic

**Provider Commands**:
- [ ] `list_generation_providers` - Show available AI providers
- [ ] `set_default_provider` - Change default provider
- [ ] `reload_config` - Hot reload configuration

**Memory Commands**:
- [ ] `search_memory` - Search facts database
- [ ] `get_memory_stats` - Show memory statistics
- [ ] `clear_memory` - Clear all facts

**Tool Commands**:
- [ ] `list_tools` - List available tools
- [ ] `get_tool_count` - Count tools

**MCP Commands**:
- [ ] `list_mcp_servers` - List MCP servers
- [ ] `get_mcp_config` - Get MCP configuration

**Skills Commands**:
- [ ] `list_skills` - List available skills

**Event Streaming** (11 event types):
- [ ] `RunAccepted` - Run started notification
- [ ] `Reasoning` - Thinking process display
- [ ] `ToolStart` - Tool execution begins
- [ ] `ToolUpdate` - Tool progress updates
- [ ] `ToolEnd` - Tool execution completes
- [ ] `ResponseChunk` - Streaming text chunks
- [ ] `RunComplete` - Run finished successfully
- [ ] `RunError` - Error occurred
- [ ] `AskUser` - User input requested
- [ ] `ReasoningBlock` - Reasoning section
- [ ] `UncertaintySignal` - Uncertainty indicator

**Platform-Specific Features**:
- [ ] Global shortcuts work (if configured)
- [ ] System tray integration works
- [ ] Window management (minimize, close) works
- [ ] Clipboard integration works

#### 3.3 Production Build Test

```bash
cd apps/desktop

# Build production bundle
pnpm tauri build

# Check output
ls src-tauri/target/release/bundle/
```

**Expected Result**: Platform-specific bundles created (.dmg for macOS, .exe for Windows, etc.)

---

## Integration Testing

### Multi-Client Scenario

Test multiple clients connecting to the same Gateway simultaneously:

1. **Start Gateway**:
   ```bash
   cargo run -p alephcore --features gateway --bin aleph-gateway -- start
   ```

2. **Connect CLI Client**:
   ```bash
   cargo run -p aleph-cli -- "Test from CLI"
   ```

3. **Connect Tauri Client**:
   ```bash
   cd apps/desktop && pnpm tauri dev
   ```

4. **Verify**:
   - [ ] Both clients connect successfully
   - [ ] Each client has independent session
   - [ ] Gateway handles concurrent requests
   - [ ] No interference between clients

### Error Handling

Test error scenarios:

1. **Gateway Not Running**:
   ```bash
   # Stop Gateway
   pkill aleph-gateway

   # Try CLI
   cargo run -p aleph-cli -- "Test"
   ```
   - [ ] CLI shows clear error message
   - [ ] Tauri shows connection error in UI

2. **Invalid Authentication**:
   ```bash
   # Corrupt auth token
   echo "invalid_token" > ~/.aleph/cli_config.toml

   # Try CLI
   cargo run -p aleph-cli -- "Test"
   ```
   - [ ] Re-authentication triggered
   - [ ] New token saved

3. **Network Interruption**:
   - [ ] Disconnect network during request
   - [ ] Client shows appropriate error
   - [ ] Reconnection works when network restored

---

## Performance Testing

### Response Time

```bash
# Measure CLI response time
time cargo run -p aleph-cli -- "Hello"
```

**Expected**: < 2 seconds for simple queries (excluding model inference time)

### Memory Usage

```bash
# Check Gateway memory
ps aux | grep aleph-gateway

# Check Tauri memory
ps aux | grep aleph-tauri
```

**Expected**:
- Gateway: ~50-200 MB (depending on loaded models)
- Tauri Client: ~100-300 MB (much less than Fat Client which was 500+ MB)

### Concurrent Connections

```bash
# Start multiple CLI instances
for i in {1..10}; do
  cargo run -p aleph-cli -- "Test $i" &
done
```

**Expected**: All requests handled successfully

---

## Regression Testing

### Functionality Parity

Verify that all features from the Fat Client still work in Thin Client:

**Core Features**:
- [x] Message sending and receiving
- [x] Streaming responses
- [x] Tool execution
- [x] Memory search
- [x] Provider switching
- [x] Configuration hot reload

**UI Features** (Tauri):
- [x] Markdown rendering
- [x] Code syntax highlighting
- [x] Copy to clipboard
- [x] Topic management
- [x] Settings panel

**Platform Features**:
- [x] Global shortcuts
- [x] System tray
- [x] Notifications
- [x] File system access

---

## Known Issues

### Warnings (Non-Critical)

1. **Cocoa API Deprecation** (macOS only):
   - Location: `apps/desktop/src-tauri/src/commands/mod.rs:49-56`
   - Impact: None (still functional)
   - Fix: Migrate to objc2-app-kit (future work)

2. **Unused Code Warnings**:
   - `GatewayState::is_initialized()` - Kept for future debugging
   - `base64_serde::deserialize()` - Placeholder for trait implementation

### Limitations

1. **Frontend Dist Requirement**:
   - Tauri requires `apps/desktop/dist/` to exist
   - Placeholder created automatically
   - Real dist built by `pnpm build`

2. **Gateway Dependency**:
   - All clients require Gateway to be running
   - No offline mode (by design)

---

## Troubleshooting

### "Connection refused" Error

**Cause**: Gateway not running

**Solution**:
```bash
cargo run -p alephcore --features gateway --bin aleph-gateway -- start
```

### "Authentication failed" Error

**Cause**: Invalid or expired token

**Solution**:
```bash
# Remove old token
rm ~/.aleph/cli_config.toml

# Reconnect (will re-authenticate)
cargo run -p aleph-cli -- "Test"
```

### Tauri Build Fails with "frontendDist doesn't exist"

**Cause**: Missing dist directory

**Solution**:
```bash
cd apps/desktop
pnpm build
```

### Gateway Port Already in Use

**Cause**: Previous Gateway instance still running

**Solution**:
```bash
# Find and kill process
lsof -ti:18789 | xargs kill -9

# Or use Gateway stop command
cargo run -p alephcore --features gateway --bin aleph-gateway -- stop
```

---

## Success Criteria

All phases are considered successful if:

- ✅ All builds complete without errors
- ✅ All tests pass
- ✅ CLI connects and processes requests
- ✅ Tauri Desktop connects and all 16 commands work
- ✅ Event streaming displays correctly
- ✅ Multiple clients can connect simultaneously
- ✅ Error handling works as expected
- ✅ Performance is acceptable (< 2s response time)
- ✅ Memory usage is reduced compared to Fat Client

---

## Next Steps

After successful testing:

1. **Production Deployment**:
   - Build release binaries
   - Create installers
   - Update distribution channels

2. **Documentation**:
   - Update user guides
   - Create migration guide for existing users
   - Document new architecture

3. **Monitoring**:
   - Set up Gateway monitoring
   - Track client connection metrics
   - Monitor error rates

---

## References

- [Phase 2 Progress Report](../PHASE2_PROGRESS.md)
- [Phase 3 Progress Report](../PHASE3_PROGRESS.md)
- [Gateway Documentation](GATEWAY.md)
- [Client SDK Documentation](../apps/shared/README.md)
