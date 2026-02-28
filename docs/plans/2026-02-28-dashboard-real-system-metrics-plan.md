# Dashboard Real System Metrics — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace all hardcoded mock data on the Dashboard System Status page with real system metrics fetched via a new `system.info` RPC handler.

**Architecture:** A new stateless RPC handler queries `sysinfo` crate for CPU/memory/disk/uptime on each request. The Leptos/WASM frontend calls this handler on page load and renders real data reactively. Follows the exact same pattern as existing `health` and `version` handlers.

**Tech Stack:** Rust (sysinfo 0.33, serde_json, tokio), Leptos 0.7 (WASM), JSON-RPC 2.0

**Design Doc:** `docs/plans/2026-02-28-dashboard-real-system-metrics-design.md`

---

### Task 1: Create `system.info` RPC Handler

**Files:**
- Create: `core/src/gateway/handlers/system_info.rs`

**Step 1: Write the test**

Add a test module at the bottom of the new file. The test verifies the handler returns a success response with all expected fields.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_system_info_response() {
        let request = JsonRpcRequest::with_id("system.info", None, json!(1));
        let response = handle(request).await;

        assert!(response.is_success());

        let result = response.result.unwrap();
        assert!(result["version"].is_string());
        assert!(result["platform"].is_string());
        assert!(result["uptime_secs"].is_u64());
        assert!(result["cpu_count"].is_u64());
        assert!(result["memory_total_bytes"].is_u64());
        assert!(result["memory_used_bytes"].is_u64());
        assert!(result["disk_total_bytes"].is_u64());
        assert!(result["disk_used_bytes"].is_u64());
        // cpu_usage_percent is f64 in JSON
        assert!(result["cpu_usage_percent"].is_f64());
    }
}
```

**Step 2: Write the handler implementation**

```rust
//! System Info Handler
//!
//! Returns real system metrics: CPU, memory, disk, uptime, platform.

use serde_json::json;
use sysinfo::{Disks, System};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Handle system.info requests
///
/// Returns a JSON object with real system metrics:
/// - `version`: Crate version from Cargo.toml
/// - `platform`: OS and architecture (e.g. "macos-aarch64")
/// - `uptime_secs`: System uptime in seconds
/// - `cpu_usage_percent`: Current CPU usage percentage
/// - `cpu_count`: Number of CPU cores
/// - `memory_used_bytes`: Used physical memory in bytes
/// - `memory_total_bytes`: Total physical memory in bytes
/// - `disk_used_bytes`: Used disk space in bytes (sum of all disks)
/// - `disk_total_bytes`: Total disk space in bytes (sum of all disks)
pub async fn handle(request: JsonRpcRequest) -> JsonRpcResponse {
    // Spawn blocking because sysinfo does synchronous I/O
    let info = tokio::task::spawn_blocking(|| {
        let mut sys = System::new_all();

        // CPU requires two refreshes with a gap for accurate reading
        sys.refresh_cpu_all();
        std::thread::sleep(std::time::Duration::from_millis(200));
        sys.refresh_cpu_all();

        let cpu_usage = sys.global_cpu_usage();
        let cpu_count = sys.cpus().len();
        let memory_used = sys.used_memory();
        let memory_total = sys.total_memory();

        // Sum all disk usage
        let disks = Disks::new_with_refreshed_list();
        let mut disk_total: u64 = 0;
        let mut disk_used: u64 = 0;
        for disk in disks.list() {
            disk_total += disk.total_space();
            disk_used += disk.total_space() - disk.available_space();
        }

        let uptime = System::uptime();

        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            "uptime_secs": uptime,
            "cpu_usage_percent": cpu_usage,
            "cpu_count": cpu_count,
            "memory_used_bytes": memory_used,
            "memory_total_bytes": memory_total,
            "disk_used_bytes": disk_used,
            "disk_total_bytes": disk_total,
        })
    })
    .await
    .unwrap_or_else(|e| json!({"error": format!("Failed to collect system info: {}", e)}));

    JsonRpcResponse::success(request.id, info)
}
```

**Step 3: Run the test**

Run: `cd core && cargo test --features gateway gateway::handlers::system_info::tests -v`

Expected: PASS — the handler should return all fields with real values from the local machine.

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/system_info.rs
git commit -m "gateway: add system.info RPC handler with real system metrics"
```

---

### Task 2: Register the Handler

**Files:**
- Modify: `core/src/gateway/handlers/mod.rs:42` (add module declaration)
- Modify: `core/src/gateway/handlers/mod.rs:154` (register handler)

**Step 1: Add module declaration**

After line 44 (`pub mod version;`), add:

```rust
pub mod system_info;
```

**Step 2: Register the handler**

After line 154 (`registry.register("version", version::handle);`), add:

```rust
registry.register("system.info", system_info::handle);
```

**Step 3: Run full handler tests**

Run: `cd core && cargo test --features gateway gateway::handlers -v`

Expected: All existing tests pass, plus the new `system_info` test.

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/mod.rs
git commit -m "gateway: register system.info handler"
```

---

### Task 3: Expand Frontend `SystemInfo` Struct

**Files:**
- Modify: `core/ui/control_plane/src/api.rs:223-228` (expand struct)

**Step 1: Replace the `SystemInfo` struct**

Replace the existing struct (lines 223-228) with:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub version: String,
    #[serde(default)]
    pub uptime_secs: u64,
    pub platform: String,
    #[serde(default)]
    pub cpu_usage_percent: f32,
    #[serde(default)]
    pub cpu_count: usize,
    #[serde(default)]
    pub memory_used_bytes: u64,
    #[serde(default)]
    pub memory_total_bytes: u64,
    #[serde(default)]
    pub disk_used_bytes: u64,
    #[serde(default)]
    pub disk_total_bytes: u64,
}
```

Note: `#[serde(default)]` on new fields ensures backward compatibility if somehow an older server responds. The old `uptime` field is renamed to `uptime_secs` to match the backend response.

**Step 2: Update `home.rs` reference**

In `core/ui/control_plane/src/views/home.rs:22`, the field `info.uptime` no longer exists. But looking at the code, home.rs only accesses `info.version` (line 26), so no change needed there.

**Step 3: Verify WASM build**

Run: `cd core/ui/control_plane && trunk build` (or whatever the project's WASM build command is — check for `trunk` or `wasm-pack`)

If the project doesn't have a standalone WASM build step, verify via: `cd core && cargo check --features gateway`

**Step 4: Commit**

```bash
git add core/ui/control_plane/src/api.rs
git commit -m "ui: expand SystemInfo struct with CPU, memory, disk fields"
```

---

### Task 4: Replace Hardcoded UI with Real Data

**Files:**
- Modify: `core/ui/control_plane/src/views/system_status.rs` (full rewrite of view body)

This is the largest task. The key changes:

1. Store `SystemInfo` in a signal instead of a string
2. Replace fake service cards with a single real Gateway status card
3. Replace fake resource metrics with real data from the `SystemInfo` signal

**Step 1: Update the signal and Effect**

Replace lines 12-13:
```rust
let system_info = RwSignal::new(None::<String>);
```
with:
```rust
use crate::api::SystemInfo;
let system_info = RwSignal::new(None::<SystemInfo>);
```

Replace lines 15-33 (the Effect that fetches system info):
```rust
Effect::new(move || {
    if state.is_connected.get() {
        let state = state.clone();
        leptos::task::spawn_local(async move {
            match SystemApi::info(&state).await {
                Ok(info) => {
                    system_info.set(Some(info));
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to fetch system info: {}", e).into());
                }
            }
        });
    } else {
        system_info.set(None);
    }
});
```

**Step 2: Replace left column (Core Services) — lines 182-212**

Replace the 4 fake service cards with a single real Gateway card + system info summary:

```rust
<div class="grid grid-cols-1 lg:grid-cols-2 gap-8">
    // Gateway Status + System Info
    <div class="space-y-6">
        <h3 class="text-xl font-semibold px-1 text-text-secondary">"System Info"</h3>
        <div class="space-y-4">
            <ServiceCard
                name="Gateway Engine"
                status=gateway_status
            />
            {move || {
                if let Some(info) = system_info.get() {
                    view! {
                        <Card class="p-5 space-y-3">
                            <div class="flex justify-between text-sm">
                                <span class="text-text-secondary">"Version"</span>
                                <span class="font-mono text-text-primary">{info.version.clone()}</span>
                            </div>
                            <div class="flex justify-between text-sm">
                                <span class="text-text-secondary">"Platform"</span>
                                <span class="font-mono text-text-primary">{info.platform.clone()}</span>
                            </div>
                            <div class="flex justify-between text-sm">
                                <span class="text-text-secondary">"Uptime"</span>
                                <span class="font-mono text-text-primary">{format_uptime(info.uptime_secs)}</span>
                            </div>
                        </Card>
                    }.into_any()
                } else {
                    view! {
                        <Card class="p-5">
                            <p class="text-text-tertiary text-sm">"Connect to gateway to see system info"</p>
                        </Card>
                    }.into_any()
                }
            }}
        </div>
    </div>

    // Resource Usage — real data
    <div class="space-y-6">
        <h3 class="text-xl font-semibold px-1 text-text-secondary">"Resource Utilization"</h3>
        {move || {
            if let Some(info) = system_info.get() {
                let cpu_percent = info.cpu_usage_percent as u32;
                let mem_percent = if info.memory_total_bytes > 0 {
                    ((info.memory_used_bytes as f64 / info.memory_total_bytes as f64) * 100.0) as u32
                } else { 0 };
                let disk_percent = if info.disk_total_bytes > 0 {
                    ((info.disk_used_bytes as f64 / info.disk_total_bytes as f64) * 100.0) as u32
                } else { 0 };

                view! {
                    <Card class="p-8 space-y-8">
                        <ResourceMetric
                            label="CPU"
                            value=format!("{}%", cpu_percent)
                            sub=format!("{} Cores", info.cpu_count)
                            color="bg-success"
                            progress=cpu_percent
                        >
                            <rect x="4" y="4" width="16" height="16" rx="2" ry="2" />
                            <rect x="9" y="9" width="6" height="6" />
                            <line x1="9" y1="1" x2="9" y2="4" />
                            <line x1="15" y1="1" x2="15" y2="4" />
                        </ResourceMetric>
                        <ResourceMetric
                            label="Memory"
                            value=format_bytes(info.memory_used_bytes)
                            sub=format!("of {}", format_bytes(info.memory_total_bytes))
                            color="bg-primary"
                            progress=mem_percent
                        >
                            <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" />
                        </ResourceMetric>
                        <ResourceMetric
                            label="Storage"
                            value=format_bytes(info.disk_used_bytes)
                            sub=format!("{} Free", format_bytes(info.disk_total_bytes.saturating_sub(info.disk_used_bytes)))
                            color="bg-primary"
                            progress=disk_percent
                        >
                            <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                            <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                        </ResourceMetric>
                    </Card>
                }.into_any()
            } else {
                view! {
                    <Card class="p-8">
                        <p class="text-text-tertiary text-sm">"Connect to gateway to see resource utilization"</p>
                    </Card>
                }.into_any()
            }
        }}
    </div>
</div>
```

**Step 3: Add helper functions and simplify `ServiceCard`**

Add at the bottom of the file (before the closing):

```rust
fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}

fn format_bytes(bytes: u64) -> String {
    const GB: f64 = 1_073_741_824.0;
    const MB: f64 = 1_048_576.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.0} MB", b / MB)
    }
}
```

Simplify `ServiceCard` — remove the `uptime` and `latency` params since we no longer have fake values for them:

```rust
#[component]
fn ServiceCard(
    name: &'static str,
    status: RwSignal<&'static str>,
) -> impl IntoView {
    let badge_variant = move || match status.get() {
        "Healthy" => BadgeVariant::Emerald,
        "Degraded" => BadgeVariant::Amber,
        _ => BadgeVariant::Red,
    };

    view! {
        <div class="bg-surface-raised border border-border p-5 rounded-2xl flex items-center justify-between group hover:border-border-strong transition-all">
            <div class="flex items-center gap-4">
                <div class=move || format!("w-2.5 h-2.5 rounded-full transition-all duration-500 {}",
                    if status.get() == "Healthy" { "bg-success" }
                    else if status.get() == "Degraded" { "bg-warning" }
                    else { "bg-danger" }
                )></div>
                <div class="font-medium text-text-primary text-sm">{name}</div>
            </div>
            <div class="w-24 text-right">
                <Badge variant=badge_variant()>
                    {move || status.get()}
                </Badge>
            </div>
        </div>
    }
}
```

Update `ResourceMetric` to accept `String` instead of `&'static str` for dynamic values:

```rust
#[component]
fn ResourceMetric(
    label: &'static str,
    value: String,
    sub: String,
    color: &'static str,
    progress: u32,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-6 group">
            <div class=format!("p-2.5 rounded-xl bg-surface-sunken text-white transition-transform group-hover:scale-110 {}", color)>
                <svg width="20" height="20" attr:class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    {children()}
                </svg>
            </div>
            <div class="flex-1">
                <div class="flex items-center justify-between mb-1.5">
                    <span class="text-xs font-medium text-text-secondary group-hover:text-text-primary transition-colors">{label}</span>
                    <span class="text-base font-bold font-mono">{value}</span>
                </div>
                <div class="w-full h-1.5 bg-border rounded-full overflow-hidden">
                    <div class=format!("h-full rounded-full transition-all duration-1000 ease-out {}", color) style=format!("width: {}%", progress)></div>
                </div>
                <div class="mt-1.5 text-[9px] text-text-tertiary font-medium uppercase tracking-wider">{sub}</div>
            </div>
        </div>
    }
}
```

**Step 4: Verify build**

Run: `cd core && cargo check --features gateway`

If WASM build is available: `cd core/ui/control_plane && trunk build`

**Step 5: Commit**

```bash
git add core/ui/control_plane/src/views/system_status.rs
git commit -m "ui: replace hardcoded system status with real metrics from system.info RPC"
```

---

### Task 5: Build WASM and Verify End-to-End

**Files:**
- Rebuild: `core/ui/control_plane/dist/` (compiled WASM + JS)

**Step 1: Rebuild the control plane UI**

Check if there's a build script:

```bash
ls Scripts/rebuild-ui.sh
```

If it exists, run it. Otherwise:

```bash
cd core/ui/control_plane && trunk build --release
```

Or if using wasm-pack:

```bash
cd core/ui/control_plane && wasm-pack build --target web --release
```

**Step 2: Start the server and verify**

```bash
cd core && cargo run --bin aleph-server --features control-plane
```

Open the dashboard in browser, navigate to System Health page. Verify:
- Gateway connection status shows real state (Healthy/Disconnected)
- CPU shows real percentage and core count
- Memory shows real used/total in GB
- Storage shows real used/free in GB
- No more fake "Agent Runtime", "Memory Vector DB", "MCP Tool Server" cards
- No more fake "Security Layer" card

**Step 3: Commit built assets**

```bash
git add core/ui/control_plane/dist/
git commit -m "ui: rebuild control plane WASM with real system metrics"
```

---

## Summary of Changes

| # | Task | Files | Estimated Effort |
|---|------|-------|-----------------|
| 1 | Create `system.info` handler | `handlers/system_info.rs` (new) | ~50 lines |
| 2 | Register handler | `handlers/mod.rs` (+2 lines) | Trivial |
| 3 | Expand `SystemInfo` struct | `api.rs` (~15 lines changed) | Small |
| 4 | Replace hardcoded UI | `system_status.rs` (~200 lines rewritten) | Medium |
| 5 | Build & verify E2E | Compiled assets | Build step |

Total: 4 files modified, 1 file created, ~5 commits.
