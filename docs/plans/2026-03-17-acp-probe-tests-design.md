# ACP Probe Tests — Design Document

**Date**: 2026-03-17
**Status**: Approved
**Scope**: Comprehensive probe tests for ACP architecture (65 tests across 7 layers)

## Background

ACP harness management system is complete (config, manager, harnesses, gateway handlers, panel UI). Existing unit tests cover ~60-70% of core logic. This probe test suite fills the gaps: cross-layer integration, real process spawn, error paths, and E2E RPC verification.

## Test Architecture

### Two-Layer Strategy

- **A Layer (P1-P6)**: Rust integration probes with mock substitutes. Fast, deterministic, broad coverage.
- **B Layer (P7)**: Real server RPC probe. Spawns Aleph server, sends WebSocket JSON-RPC requests.

### Mock Strategy

- **Trait Mock (MockAcpHarness)**: Implements `AcpHarness` trait with response queue, availability control, call tracking. Used in P2/P4/P5.
- **Mock Scripts**: Shell/Python scripts simulating CLI tools. Tests real process spawn + output parsing in P3/P6.

### No Panel Testing

Panel UI tests are out of scope. Coverage relies on Core-layer RPC probe verification.

## File Structure

```
tests/acp_probe.rs                  // Entry point (mod declarations)
tests/acp_probe/
├── harness.rs                           // Test harness (Builder pattern)
├── mock_harness.rs                      // MockAcpHarness (trait-level mock)
├── mock_scripts/
│   ├── mock_claude.sh                   // JSON output (oneshot)
│   ├── mock_codex.sh                    // Plain text output (oneshot)
│   ├── mock_gemini_acp.py              // NativeAcp NDJSON protocol
│   ├── mock_crash.sh                    // Exit code 1
│   ├── mock_timeout.sh                  // sleep 300 (hang)
│   ├── mock_env_echo.sh                // Echo env vars (test env passing)
│   └── mock_cwd_echo.sh               // Echo $PWD (test cwd setting)
├── p1_config_and_presets.rs            // 7 tests
├── p2_manager_lifecycle.rs             // 10 tests
├── p3_custom_harness.rs                // 10 tests
├── p4_rpc_handlers.rs                  // 13 tests
├── p5_tool_execution.rs               // 7 tests
├── p6_error_paths.rs                   // 8 tests
└── p7_rpc_server_probe.rs             // 10 tests
```

## Part 1: Mock Infrastructure

### MockAcpHarness

```rust
pub struct MockAcpHarness {
    id: String,
    display_name: String,
    mode: HarnessMode,
    available: AtomicBool,
    response_queue: Mutex<VecDeque<Result<String>>>,
    default_response: String,
    call_count: AtomicU64,
    last_prompt: Mutex<Option<String>>,
}
```

Behavior control:
- `enqueue_response(result)` — FIFO response queue
- `set_available(bool)` — control is_available()
- `set_failing()` — all prompts return error
- Query: `call_count()`, `last_prompt()`, `was_called()`

### Mock Scripts

| Script | Purpose | Output |
|--------|---------|--------|
| `mock_claude.sh` | Oneshot JSON | `{"type":"result","result":"echo: $*"}` |
| `mock_codex.sh` | Oneshot plaintext | `codex response: $*` |
| `mock_gemini_acp.py` | NativeAcp NDJSON | Full protocol (initialize → session/new → prompt) |
| `mock_crash.sh` | Process failure | `exit 1` |
| `mock_timeout.sh` | Hang | `sleep 300` |
| `mock_env_echo.sh` | Env var passing | Echo `$TEST_VAR` |
| `mock_cwd_echo.sh` | Cwd verification | Echo `$PWD` |

All scripts `chmod +x` and located in `tests/acp_probe/mock_scripts/`.

## Part 2: P1 — Config & Presets (7 tests, pure data)

| Test | Assertion |
|------|-----------|
| `p1_01_preset_defaults_complete` | 3 presets have correct executable/args/mode/output_format |
| `p1_02_all_presets_returns_three` | `all_presets()` returns 3 entries with hyphenated keys |
| `p1_03_is_preset_id` | "claude-code"/"codex"/"gemini" → true; custom → false |
| `p1_04_harness_mode_serde_roundtrip` | Serialize + deserialize TOML/JSON preserves value |
| `p1_05_output_format_serde_roundtrip` | PlainText and Json variants survive roundtrip |
| `p1_06_config_merge_user_override` | User config overlays preset defaults correctly |
| `p1_07_default_values_sensible` | Default timeout=300, enabled=true |

## Part 3: P2 — Manager Lifecycle (10 tests, trait mock)

| Test | Assertion |
|------|-----------|
| `p2_01_from_entries_registers_all` | 3 presets + 1 custom all registered |
| `p2_02_disabled_entry_not_registered` | enabled=false excluded from harness_ids() |
| `p2_03_register_custom_harness` | Dynamic registration works |
| `p2_04_unregister_custom_harness` | Removal works |
| `p2_05_unregister_preset_rejected` | Delete "claude-code" → Err |
| `p2_06_update_harness_replaces` | get_config() returns new config after update |
| `p2_07_update_kills_active_session` | Update NativeAcp harness kills old session |
| `p2_08_prompt_routes_to_correct_mode` | Oneshot → execute_oneshot, NativeAcp → session.prompt |
| `p2_09_available_harnesses_filters` | Only is_available()=true returned |
| `p2_10_list_configs_returns_all` | Includes disabled configs |

## Part 4: P3 — CustomHarness (10 tests, mock scripts)

| Test | Assertion |
|------|-----------|
| `p3_01_oneshot_plaintext` | mock_codex.sh output trimmed correctly |
| `p3_02_oneshot_json_extract_field` | mock_claude.sh JSON "result" field extracted |
| `p3_03_oneshot_json_missing_field` | Missing field → full JSON string |
| `p3_04_oneshot_json_invalid_json` | Non-JSON → plaintext fallback |
| `p3_05_oneshot_with_env_vars` | Env vars passed to subprocess |
| `p3_06_oneshot_with_custom_cwd` | Working directory set correctly |
| `p3_07_oneshot_process_crash` | Exit 1 → AlephError::tool |
| `p3_08_oneshot_executable_not_found` | Missing executable → error |
| `p3_09_build_config_maps_all_fields` | All AcpHarnessEntry fields → HarnessConfig |
| `p3_10_is_available_checks_version` | --version exit 0 → true |

## Part 5: P4 — RPC Handlers (13 tests, mock manager + config)

| Test | Assertion |
|------|-----------|
| `p4_01_list_returns_all_with_availability` | Presets + custom with available field |
| `p4_02_list_merges_preset_defaults` | Unconfigured presets appear with defaults |
| `p4_03_get_existing_harness` | Returns correct AcpHarnessInfo |
| `p4_04_get_nonexistent_returns_error` | INVALID_PARAMS error |
| `p4_05_create_custom_persists_to_config` | Config.acp.harnesses contains new entry |
| `p4_06_create_preset_id_rejected` | "claude-code" → error |
| `p4_07_create_invalid_id_rejected` | Uppercase/spaces/empty → INVALID_PARAMS |
| `p4_08_update_saves_and_broadcasts` | Config updated + EventBus receives "config.acp.changed" |
| `p4_09_delete_custom_removes_from_config` | Entry removed from config |
| `p4_10_delete_preset_rejected` | "gemini" → error |
| `p4_11_test_returns_timing` | success + duration_ms > 0 |
| `p4_12_set_enabled_toggle` | Enabled state toggled in config |
| `p4_13_presets_returns_three_defaults` | 3 preset templates returned |

## Part 6: P5 — Tool Execution (7 tests, trait mock)

| Test | Assertion |
|------|-----------|
| `p5_01_claude_code_tool_calls_manager` | Prompt routed to "claude-code" |
| `p5_02_codex_tool_calls_manager` | Prompt routed to "codex" |
| `p5_03_gemini_tool_calls_manager` | Prompt routed to "gemini" |
| `p5_04_tool_returns_harness_output` | Return contains harness name + result |
| `p5_05_tool_unavailable_harness_error` | Friendly error on unavailable |
| `p5_06_switch_tool_validates_target` | Unknown target → error |
| `p5_07_tool_cwd_defaults_to_home` | No cwd → home_dir fallback |

## Part 7: P6 — Error Paths (8 tests, mixed mocks)

| Test | Assertion |
|------|-----------|
| `p6_01_oneshot_timeout` | mock_timeout.sh + short timeout → error, process killed |
| `p6_02_native_acp_session_crash_respawn` | Dead session → auto-respawn |
| `p6_03_prompt_to_dead_session` | Auto respawn + retry |
| `p6_04_initialize_timeout` | NativeAcp init timeout → error |
| `p6_05_malformed_ndjson_response` | Invalid JSON → skip, continue |
| `p6_06_concurrent_register_unregister` | No panic, no deadlock |
| `p6_07_shutdown_all_kills_sessions` | All sessions terminated |
| `p6_08_manager_prompt_unknown_harness` | "Unknown ACP harness" error |

## Part 8: P7 — Real Server RPC Probe (10 tests, E2E)

Uses `provider_rpc_probe` pattern: OnceCell singleton server, random port, WebSocket RPC.

All tests marked `#[serial]`.

| Test | Assertion |
|------|-----------|
| `p7_01_list_returns_presets` | ≥3 presets, valid AcpHarnessInfo schema |
| `p7_02_get_preset` | Correct display_name + mode for "codex" |
| `p7_03_get_nonexistent` | JSON-RPC error response |
| `p7_04_create_update_delete_cycle` | Full CRUD lifecycle in one test |
| `p7_05_create_invalid_id` | Error on bad ID format |
| `p7_06_delete_preset_rejected` | Error on preset deletion |
| `p7_07_presets_returns_defaults` | 3 preset templates |
| `p7_08_set_enabled_toggle` | Toggle verified via subsequent list |
| `p7_09_test_harness_returns_result` | Valid AcpTestResult structure (success may be false) |
| `p7_10_config_persistence` | Created harness survives get verification |

## Execution

```bash
# All ACP probe tests
cargo test -p alephcore --test acp_probe

# Specific layer
cargo test -p alephcore --test acp_probe p1_
cargo test -p alephcore --test acp_probe p7_ -- --test-threads=1

# Quick smoke (P1-P3 only, no server)
cargo test -p alephcore --test acp_probe p1_ p2_ p3_
```

## Summary

| Layer | Tests | Mock Strategy | Speed |
|-------|-------|---------------|-------|
| P1 Config | 7 | None | <1s |
| P2 Manager | 10 | Trait mock | <2s |
| P3 CustomHarness | 10 | Mock scripts | ~5s |
| P4 RPC Handlers | 13 | Trait mock + Config | <3s |
| P5 Tool Execution | 7 | Trait mock | <2s |
| P6 Error Paths | 8 | Mixed | ~10s |
| P7 Real Server | 10 | Real server | ~30s |
| **Total** | **65** | | **~50s** |
