# Scheduled Tasks Panel Design

> Date: 2026-03-04
> Status: Approved
> Scope: UI layer only (Dashboard view + API wrapper + route integration)

## Overview

Add a "Scheduled Tasks" view to the Dashboard mode, allowing users to view, create, edit, and delete cron jobs through a left-right split-pane layout.

## Navigation & Routing

- **Entry point**: `DashboardSidebar` → new `SidebarItem` at `href="/dashboard/cron"`, label "Scheduled Tasks", clock icon
- **Route**: `DashboardRouter` → `"/dashboard/cron" => CronView`
- Follows existing pattern: identical to Overview / Agent Trace / System Health / Memory Vault

### Files affected

| File | Change |
|------|--------|
| `components/dashboard_sidebar.rs` | Add `SidebarItem` for `/dashboard/cron` |
| `app.rs` | Add match arm in `DashboardRouter` |
| `views/cron.rs` (new) | Main `CronView` component |

## View Layout: CronView

Left-right split pane, matching `RoutingRulesView` pattern.

### Left Pane — Job List

- Top: `[+ New Job]` button
- Each item shows:
  - Status indicator (● enabled green / ○ disabled gray)
  - Job name
  - Schedule summary (e.g., "9:00 daily", "Every 30min")
  - Next run time or last run result
- Click to select (highlight active)

### Right Pane — Editor + History

**Upper: Editor Form** (using `SettingsSection` + `FormField` components)

| Field | Control | Notes |
|-------|---------|-------|
| Name | TextInput | Job name |
| Schedule Kind | Select (Cron/Every/At) | Schedule type |
| Schedule | TextInput | Contextual: cron expr / interval ms / timestamp |
| Agent | Select | Available agent list |
| Prompt | Textarea | Message sent to agent |
| Timezone | Select | Optional timezone |
| Tags | TextInput | Comma-separated |
| Enabled | Toggle | Enable/disable |

Action buttons: `[Save]` `[Delete]` `[Run Now]`

**Lower: Execution History**

Table of recent 10 `JobRun` records:
- Status (✓ success / ✗ failed / ⏱ timeout)
- Execution time
- Duration
- Error message (on failure)

### Empty States

- No job selected → "Select a job to view details, or create a new one."
- No jobs exist → Full-screen empty state with create button

## RPC Interface

### Methods

| Method | Params | Returns | Description |
|--------|--------|---------|-------------|
| `cron.list` | `{filter?: {enabled?, schedule_kind?}}` | `{jobs: CronJob[]}` | List all jobs |
| `cron.get` | `{job_id}` | `CronJob` | Get single job |
| `cron.create` | `{name, schedule, agent_id, prompt, ...}` | `CronJob` | Create job |
| `cron.update` | `{job_id, ...patch}` | `CronJob` | Update job |
| `cron.delete` | `{job_id}` | `{deleted: true}` | Delete job |
| `cron.run` | `{job_id}` | `{triggered, status}` | Manual trigger (existing) |
| `cron.runs` | `{job_id, limit?}` | `{runs: JobRun[]}` | Execution history |
| `cron.toggle` | `{job_id, enabled}` | `CronJob` | Quick enable/disable |

### Data Flow

```
UI (Leptos/WASM) → CronApi → rpc_call() → WebSocket → Gateway Handler → CronService → SQLite
```

## UI API Layer

New file: `api/cron.rs`

```rust
pub struct CronApi;

impl CronApi {
    pub async fn list(state: &DashboardState) -> Result<Vec<CronJobInfo>, String>;
    pub async fn get(state: &DashboardState, job_id: &str) -> Result<CronJobInfo, String>;
    pub async fn create(state: &DashboardState, job: CreateCronJob) -> Result<CronJobInfo, String>;
    pub async fn update(state: &DashboardState, job_id: &str, patch: UpdateCronJob) -> Result<CronJobInfo, String>;
    pub async fn delete(state: &DashboardState, job_id: &str) -> Result<(), String>;
    pub async fn runs(state: &DashboardState, job_id: &str, limit: i32) -> Result<Vec<JobRunInfo>, String>;
    pub async fn toggle(state: &DashboardState, job_id: &str, enabled: bool) -> Result<CronJobInfo, String>;
    pub async fn run_now(state: &DashboardState, job_id: &str) -> Result<Value, String>;
}
```

### UI DTOs

```rust
// Lightweight types for UI rendering (subset of core CronJob)
pub struct CronJobInfo {
    pub id: String,
    pub name: String,
    pub schedule: String,
    pub schedule_kind: String,
    pub agent_id: String,
    pub prompt: String,
    pub enabled: bool,
    pub timezone: Option<String>,
    pub tags: Vec<String>,
    pub next_run_at: Option<i64>,
    pub last_run_at: Option<i64>,
}

pub struct CreateCronJob {
    pub name: String,
    pub schedule: String,
    pub schedule_kind: String,
    pub agent_id: String,
    pub prompt: String,
    pub enabled: bool,
    pub timezone: Option<String>,
    pub tags: Vec<String>,
}

pub struct UpdateCronJob {
    pub name: Option<String>,
    pub schedule: Option<String>,
    pub schedule_kind: Option<String>,
    pub agent_id: Option<String>,
    pub prompt: Option<String>,
    pub enabled: Option<bool>,
    pub timezone: Option<String>,
    pub tags: Option<Vec<String>>,
}

pub struct JobRunInfo {
    pub id: String,
    pub status: String,
    pub started_at: i64,
    pub duration_ms: u64,
    pub error: Option<String>,
}
```

## Scope Boundary

**In scope (this design)**:
- UI view components (CronView, JobList, JobEditor, RunHistory)
- API wrapper (CronApi)
- Dashboard sidebar + router integration
- Gateway RPC handler stubs (expand existing cron.rs with CRUD stubs)

**Out of scope**:
- CronService lifecycle integration (handlers return stub data)
- Real-time event subscriptions
- Advanced fields (chaining, delivery, templates)
- Cron expression validation UI

## Component Structure

```
views/cron.rs
├── CronView           — Main container (left-right split)
├── JobList            — Left pane: scrollable job list
├── JobListItem        — Individual job item in list
├── JobEditor          — Right pane upper: edit form
└── RunHistory         — Right pane lower: execution history table
```

## Design Decisions

1. **Dashboard over Settings** — Cron jobs are operational (monitoring/execution), not configuration
2. **Left-right split** — Proven pattern in the codebase (RoutingRulesView), efficient for managing multiple items
3. **Basic fields only** — Cover 80% use case, avoid overwhelming UI; advanced fields can be added later
4. **Stub RPC handlers** — Unblock UI development, CronService integration is a separate task
5. **Single file** — All components in `views/cron.rs` until complexity warrants splitting
