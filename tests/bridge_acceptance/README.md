# Bridge Acceptance Tests

Acceptance test suite for the Desktop Bridge (Tauri). These tests verify that the
Tauri bridge provides functional parity with the deprecated macOS Swift app before
the Swift app is removed.

## Prerequisites

Required tools:

- `socat` — UDS communication (`brew install socat`)
- `jq` — JSON processing (`brew install jq`)
- `bc` — arithmetic
- `python3` — high-resolution timing

The bridge must be running and listening on `~/.aleph/desktop.sock` (or the path
set in `ALEPH_SOCKET_PATH`).

## Quick Start

```bash
# Run all test suites (skip stability by default)
./run_all.sh

# Run all including stability tests
./run_all.sh --include-stability

# Run individual suites
./test_functional_parity.sh
./test_performance.sh
./test_e2e_flow.sh
./test_stability.sh
```

## Test Suites

| Script | Tests | Description |
|--------|-------|-------------|
| `test_functional_parity.sh` | F1-F12 | Functional parity with Swift app |
| `test_e2e_flow.sh` | E1-E6 | Server-bridge lifecycle (manages own processes) |
| `test_performance.sh` | P1-P5 | Latency benchmarks |
| `test_stability.sh` | S1-S4 | Long-running stability and stress tests |
| `manual_checklist.md` | M1-M5 | Manual verification steps |

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ALEPH_SOCKET_PATH` | `~/.aleph/desktop.sock` | Bridge UDS socket path |
| `ALEPH_SERVER_BIN` | (none) | Path to `aleph` binary (required for E2E tests) |
| `STABILITY_LONG_RUN_SECS` | `300` | Duration for S1 memory leak check |
| `STABILITY_HOTKEY_CYCLES` | `100` | Cycles for S2 hotkey test |
| `STABILITY_UDS_CYCLES` | `10` | Cycles for S3 connection cycling |
| `STABILITY_STRESS_DURATION_SECS` | `300` | Duration for S4 high-frequency stress |
| `STABILITY_STRESS_RPS` | `10` | Requests per second for S4 |

## Architecture

All scripts source `lib.sh` for shared infrastructure:

- `send_rpc()` — send JSON-RPC 2.0 over UDS via socat
- `assert_json_has/eq/gt()` — JSON response assertions
- `run_test()` / `skip_test()` — test lifecycle with colored output
- `benchmark_rpc()` — latency benchmarking with p95 calculation
- `print_summary()` — final pass/fail report

## Protocol Reference

The bridge listens on a Unix Domain Socket and speaks JSON-RPC 2.0:

```
Methods: desktop.ping, desktop.screenshot, desktop.ocr, desktop.ax_tree,
         desktop.click, desktop.type_text, desktop.key_combo,
         desktop.launch_app, desktop.window_list, desktop.focus_window,
         desktop.canvas_show, desktop.canvas_hide, desktop.canvas_update

Error codes: -32700 (parse), -32601 (method not found),
             -32603 (internal), -32000 (not implemented)
```
