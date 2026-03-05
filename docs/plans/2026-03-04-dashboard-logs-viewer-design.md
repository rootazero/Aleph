# Dashboard Logs Viewer Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a Logs page to the Dashboard panel displaying Aleph server logs with level filtering, configurable line count, and manual refresh.

**Architecture:** Pure frontend approach — leverage existing `daemon.logs` Gateway RPC, fix its file-matching bug, add Leptos view. No new backend infrastructure needed.

**Tech Stack:** Leptos/WASM (frontend), Rust (backend bug fix), Tailwind CSS (styling), WebSocket JSON-RPC (communication)

---

## Task 1: Fix `find_latest_log()` file matching bug

**Files:**
- Modify: `core/src/gateway/handlers/daemon_control.rs:132-145`

**Step 1: Write a failing test**

Add to the existing `#[cfg(test)] mod tests` block in `daemon_control.rs`:

```rust
#[test]
fn find_latest_log_matches_dated_files() {
    let dir = std::env::temp_dir().join("aleph_log_test");
    let _ = std::fs::create_dir_all(&dir);

    // Create a file matching real naming: aleph-server.log.2026-03-04
    let dated = dir.join("aleph-server.log.2026-03-04");
    std::fs::write(&dated, "test log line").unwrap();

    let result = find_latest_log(&dir);
    assert!(result.is_some(), "Should find dated log file");
    assert!(result.unwrap().file_name().unwrap().to_string_lossy().contains("aleph-server"));

    // Cleanup
    let _ = std::fs::remove_dir_all(&dir);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib daemon_control::tests::find_latest_log_matches_dated_files`
Expected: FAIL — current code checks `extension() == "log"` which won't match `aleph-server.log.2026-03-04` (extension is `2026-03-04`)

**Step 3: Fix `find_latest_log()`**

Replace lines 132-145 of `daemon_control.rs`:

```rust
/// Find the most recent log file in the directory
fn find_latest_log(dir: &std::path::Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("aleph-") && name.contains(".log")
        })
        .max_by_key(|e| e.metadata().ok().and_then(|m| m.modified().ok()))
        .map(|e| e.path())
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib daemon_control::tests`
Expected: ALL PASS (3 tests)

**Step 5: Commit**

```
daemon: fix find_latest_log file matching for dated log files
```

---

## Task 2: Add `LogsApi` to the API layer

**Files:**
- Modify: `core/ui/control_plane/src/api.rs` (append after WorkspaceApi, ~line 1721)

**Step 1: Add `LogsResponse` type and `LogsApi` implementation**

Append to `api.rs` after the `WorkspaceApi` impl block:

```rust
// ============================================================================
// Logs API
// ============================================================================

#[derive(Debug, Clone, Deserialize)]
pub struct LogsResponse {
    #[serde(default)]
    pub logs: Vec<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub total_lines: usize,
}

pub struct LogsApi;

impl LogsApi {
    /// Fetch recent log lines from the server
    pub async fn fetch(
        state: &DashboardState,
        lines: usize,
        level: Option<&str>,
    ) -> Result<LogsResponse, String> {
        let mut params = serde_json::json!({ "lines": lines });
        if let Some(lvl) = level {
            params["level"] = serde_json::Value::String(lvl.to_string());
        }
        let result = state.rpc_call("daemon.logs", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse logs: {}", e))
    }
}
```

**Step 2: Verify WASM compilation**

Run: `cargo check -p aleph-dashboard --target wasm32-unknown-unknown`
Expected: compiles with no errors

**Step 3: Commit**

```
dashboard: add LogsApi for daemon.logs RPC
```

---

## Task 3: Create Logs view component

**Files:**
- Create: `core/ui/control_plane/src/views/logs.rs`

**Step 1: Create the Logs view**

Create `core/ui/control_plane/src/views/logs.rs` with the full component:

```rust
use leptos::prelude::*;
use crate::components::ui::*;
use crate::context::DashboardState;
use crate::api::{LogsApi, LogsResponse};

/// Return a Tailwind text color class based on the log level found in the line.
fn log_line_color(line: &str) -> &'static str {
    if line.contains(" ERROR ") {
        "text-danger"
    } else if line.contains(" WARN ") {
        "text-warning"
    } else if line.contains(" DEBUG ") || line.contains(" TRACE ") {
        "text-text-tertiary"
    } else {
        "text-text-secondary"
    }
}

#[component]
pub fn Logs() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Reactive state
    let logs_data = RwSignal::new(None::<LogsResponse>);
    let error_msg = RwSignal::new(None::<String>);
    let is_loading = RwSignal::new(false);
    let selected_level = RwSignal::new("all".to_string());
    let selected_lines = RwSignal::new(100usize);

    // Fetch logs action
    let fetch_logs = move || {
        let state = state.clone();
        leptos::task::spawn_local(async move {
            is_loading.set(true);
            error_msg.set(None);

            let level = {
                let l = selected_level.get_untracked();
                if l == "all" { None } else { Some(l) }
            };
            let lines = selected_lines.get_untracked();

            match LogsApi::fetch(&state, lines, level.as_deref()).await {
                Ok(response) => {
                    logs_data.set(Some(response));
                }
                Err(e) => {
                    error_msg.set(Some(e));
                }
            }
            is_loading.set(false);
        });
    };

    // Auto-fetch when connected
    let fetch_on_connect = fetch_logs.clone();
    Effect::new(move || {
        if state.is_connected.get() {
            fetch_on_connect();
        } else {
            logs_data.set(None);
        }
    });

    // Refresh handler
    let fetch_on_click = fetch_logs.clone();
    let handle_refresh = move |_| {
        fetch_on_click();
    };

    // Level change handler
    let fetch_on_level = fetch_logs.clone();
    let handle_level_change = move |ev: web_sys::Event| {
        let target = event_target::<web_sys::HtmlSelectElement>(&ev);
        selected_level.set(target.value());
        fetch_on_level();
    };

    // Lines change handler
    let handle_lines_change = move |ev: web_sys::Event| {
        let target = event_target::<web_sys::HtmlSelectElement>(&ev);
        if let Ok(n) = target.value().parse::<usize>() {
            selected_lines.set(n);
            fetch_logs();
        }
    };

    view! {
        <div class="p-8 max-w-7xl mx-auto space-y-6">
            // Header
            <header class="flex items-center justify-between">
                <div>
                    <h2 class="text-3xl font-bold tracking-tight mb-2 flex items-center gap-3 text-text-primary">
                        <svg width="32" height="32" attr:class="w-8 h-8 text-primary" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
                            <polyline points="14 2 14 8 20 8" />
                            <line x1="16" y1="13" x2="8" y2="13" />
                            <line x1="16" y1="17" x2="8" y2="17" />
                            <polyline points="10 9 9 9 8 9" />
                        </svg>
                        "Server Logs"
                    </h2>
                    <p class="text-text-secondary">"View recent log output from Aleph Core."</p>
                </div>
                <Button
                    on:click=handle_refresh
                    variant=ButtonVariant::Secondary
                    disabled=Signal::derive(move || is_loading.get() || !state.is_connected.get())
                >
                    {move || if is_loading.get() { "Loading..." } else { "Refresh" }}
                </Button>
            </header>

            // Error banner
            {move || error_msg.get().map(|e| view! {
                <div class="bg-danger-subtle border border-danger/20 rounded-xl p-4 text-sm text-danger">
                    <strong>"Error: "</strong> {e}
                </div>
            })}

            // Controls bar
            <Card class="p-4">
                <div class="flex items-center gap-6 flex-wrap">
                    // Level filter
                    <div class="flex items-center gap-2">
                        <label class="text-xs font-medium text-text-tertiary uppercase tracking-wider">"Level"</label>
                        <select
                            on:change=handle_level_change
                            class="bg-surface-sunken border border-border rounded-lg px-3 py-1.5 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/50"
                        >
                            <option value="all">"All"</option>
                            <option value="error">"Error"</option>
                            <option value="warn">"Warn"</option>
                            <option value="info">"Info"</option>
                            <option value="debug">"Debug"</option>
                            <option value="trace">"Trace"</option>
                        </select>
                    </div>

                    // Lines count
                    <div class="flex items-center gap-2">
                        <label class="text-xs font-medium text-text-tertiary uppercase tracking-wider">"Lines"</label>
                        <select
                            on:change=handle_lines_change
                            class="bg-surface-sunken border border-border rounded-lg px-3 py-1.5 text-sm text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/50"
                        >
                            <option value="50">"50"</option>
                            <option value="100" selected>"100"</option>
                            <option value="200">"200"</option>
                            <option value="500">"500"</option>
                        </select>
                    </div>

                    // File path display
                    {move || logs_data.get().and_then(|d| d.file).map(|f| view! {
                        <div class="flex items-center gap-2 ml-auto">
                            <span class="text-xs text-text-tertiary font-mono">{f}</span>
                        </div>
                    })}
                </div>
            </Card>

            // Log content
            {move || {
                if !state.is_connected.get() {
                    view! {
                        <Card class="p-12 text-center">
                            <p class="text-text-tertiary">"Connect to Gateway to view logs"</p>
                        </Card>
                    }.into_any()
                } else if let Some(data) = logs_data.get() {
                    if data.logs.is_empty() {
                        view! {
                            <Card class="p-12 text-center">
                                <p class="text-text-tertiary">"No log entries found"</p>
                            </Card>
                        }.into_any()
                    } else {
                        view! {
                            <Card class="overflow-hidden">
                                <div class="max-h-[600px] overflow-y-auto p-4 bg-surface-sunken">
                                    <pre class="font-mono text-xs leading-relaxed whitespace-pre-wrap break-all">
                                        {data.logs.into_iter().map(|line| {
                                            let color = log_line_color(&line);
                                            view! {
                                                <div class=format!("{} hover:bg-surface-raised/50 px-1 -mx-1 rounded", color)>
                                                    {line}
                                                </div>
                                            }
                                        }).collect_view()}
                                    </pre>
                                </div>
                            </Card>
                        }.into_any()
                    }
                } else {
                    view! {
                        <Card class="p-12 text-center">
                            <p class="text-text-tertiary">"Loading..."</p>
                        </Card>
                    }.into_any()
                }
            }}
        </div>
    }
}
```

**Step 2: Verify WASM compilation**

Run: `cargo check -p aleph-dashboard --target wasm32-unknown-unknown`
Expected: compiles (will fail until Task 4 wires the module)

---

## Task 4: Wire up routing and navigation

**Files:**
- Modify: `core/ui/control_plane/src/views/mod.rs:1`
- Modify: `core/ui/control_plane/src/app.rs:7,179-182`
- Modify: `core/ui/control_plane/src/components/dashboard_sidebar.rs:40-43`

**Step 1: Register the module**

In `views/mod.rs`, add after `pub mod cron;` (line 7):

```rust
pub mod logs;
```

**Step 2: Add route in `DashboardRouter`**

In `app.rs`, add the import at line 12 (after the `CronView` import):

```rust
use crate::views::logs::Logs;
```

In `DashboardRouter` (line 182), add before the `_ =>` catch-all:

```rust
"/dashboard/logs" => view! { <Logs /> }.into_any(),
```

**Step 3: Add sidebar item**

In `dashboard_sidebar.rs`, add after the Scheduled Tasks `SidebarItem` (before `</nav>`):

```rust
<SidebarItem href="/dashboard/logs" label="Server Logs">
    <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
    <polyline points="14 2 14 8 20 8" />
    <line x1="16" y1="13" x2="8" y2="13" />
    <line x1="16" y1="17" x2="8" y2="17" />
    <polyline points="10 9 9 9 8 9" />
</SidebarItem>
```

**Step 4: Verify WASM compilation**

Run: `cargo check -p aleph-dashboard --target wasm32-unknown-unknown`
Expected: compiles with no errors

**Step 5: Commit**

```
dashboard: add Server Logs page with level filtering and refresh
```

---

## Task 5: Build and verify

**Step 1: Run full core check**

Run: `cargo check -p alephcore`
Expected: no errors

**Step 2: Run backend tests**

Run: `cargo test -p alephcore --lib daemon_control::tests`
Expected: ALL PASS

**Step 3: Build WASM dashboard**

Run: `cd core/ui/control_plane && trunk build` (or the project's WASM build command)
Expected: builds successfully

**Step 4: Final commit (if any fixups needed)**

```
dashboard: logs viewer polish
```
