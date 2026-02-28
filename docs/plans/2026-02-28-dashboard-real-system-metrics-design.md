# Dashboard Real System Metrics Design

> **Date**: 2026-02-28
> **Status**: Approved
> **Scope**: Replace hardcoded dashboard data with real system metrics

---

## Problem

The Control Plane Dashboard's System Status page displays 100% hardcoded mock data:
- Service cards show fake uptime ("12d 4h") and latency ("14ms")
- Resource metrics show fake values (CPU "24%", Memory "4.2 GB", Storage "128 GB")
- Home page's `SystemApi::info()` calls `system.info` RPC, but the handler doesn't exist

## Decision

**Approach A: Direct sysinfo call in handler** — simplest, minimal changes, consistent with existing handler patterns (health, version, echo).

## Design

### 1. Backend: New `system.info` RPC Handler

**File**: `core/src/gateway/handlers/system_info.rs`

RPC method `system.info` returns:

```json
{
  "version": "0.1.0",
  "platform": "macos-aarch64",
  "uptime_secs": 123456,
  "cpu_usage_percent": 24.5,
  "cpu_count": 16,
  "memory_used_bytes": 4509715456,
  "memory_total_bytes": 17179869184,
  "disk_used_bytes": 137438953472,
  "disk_total_bytes": 1000204886016
}
```

Implementation:
- `sysinfo::System` (already in Cargo.toml v0.33) for CPU, memory, disk
- `env!("CARGO_PKG_VERSION")` for version
- `std::env::consts::{OS, ARCH}` for platform
- `sysinfo::System::uptime()` for system uptime
- CPU: `refresh_cpu_all()` → 200ms sleep → `refresh_cpu_all()` → `global_cpu_usage()`

### 2. Handler Registration

In `handlers/mod.rs`:
- Add `pub mod system_info;`
- Register `"system.info"` → `system_info::handle`

### 3. Frontend API: Expand `SystemInfo` Struct

In `api.rs`, expand `SystemInfo`:

```rust
pub struct SystemInfo {
    pub version: String,
    pub uptime_secs: u64,
    pub platform: String,
    pub cpu_usage_percent: f32,
    pub cpu_count: usize,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
}
```

### 4. Frontend UI: Replace Hardcoded Data

**Left column (Core Services)**: Simplify to show Gateway connection status only (the one real data point). Remove fake Agent Runtime / Memory Vector DB / MCP Tool Server cards.

**Right column (Resource Utilization)**: Replace with real data:
- **CPU** → `cpu_usage_percent`%, `cpu_count` Cores
- **Memory** → used/total (auto-format to GB), percentage progress bar
- **Storage** → used/total (auto-format to GB), percentage progress bar
- Remove fake "Security Layer" card

### 5. Data Flow

```
User opens System Status page
  → Effect detects connected state
  → Calls SystemApi::info() → RPC "system.info"
  → Gateway handler queries sysinfo crate
  → Returns JSON → Deserialize to SystemInfo
  → Signals update → UI renders real data reactively
```

## Files Changed

| File | Change |
|------|--------|
| `core/src/gateway/handlers/system_info.rs` | **New** — system.info RPC handler |
| `core/src/gateway/handlers/mod.rs` | Register new handler |
| `core/ui/control_plane/src/api.rs` | Expand SystemInfo struct |
| `core/ui/control_plane/src/views/system_status.rs` | Replace hardcoded UI with real data |

## Non-Goals

- Real-time WebSocket push (future work)
- Per-service health monitoring (Agent Runtime, MCP, etc.)
- Historical trend charts
- Perception Layer integration
