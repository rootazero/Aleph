# Panel UI Memory Management Adaptation

> **Status**: Draft
> **Date**: 2026-10-18
> **Related**: [Markdown Storage Design](2026-10-18-markdown-storage-design.md)

---

## 1. Overview

### 1.1 Current State

**Existing Component**: `interfaces/webchat/src/platform/wide/views/settings/memory.rs`

Currently provides:
- Memory system configuration (enable/disable, vector DB)
- Compression settings
- Retrieval pipeline tuning (RRF, BM25, salience scoring)
- Fact decay, dreaming, reflection settings
- **Retrieval Debug Panel** — test queries with trace visualization

**Gap**: No UI for browsing/viewing raw session data (transcripts, attachments, archives).

### 1.2 Design Goals

**New Capabilities**:
1. **Session Browser** — list and filter sessions by agent/workspace/date/tags
2. **Transcript Viewer** — render Markdown transcripts with proper formatting
3. **Attachment Gallery** — view images/documents/videos stored in sessions
4. **Archive Management** — view archived sessions, restore if needed
5. **Session Search** — full-text search across transcripts

**Non-goals**:
- ❌ Editing transcripts (read-only)
- ❌ Manual session creation (system-generated only)
- ❌ Attachment upload outside of sessions

---

## 2. UI Component Architecture

### 2.1 Navigation Structure

**Settings Menu** (existing structure extended):

```
Settings
├── General
├── Agents
├── Models
├── Memory                  ← Current config page (KEEP)
│   ├── Configuration       ← Existing knobs
│   └── Sessions            ← NEW: Session browser
├── Extensions
└── ...
```

**New Route**: `/settings/memory/sessions`

### 2.2 Component Hierarchy

```
MemoryView (existing)
└── [Configuration panels...]

SessionsView (NEW)
├── SessionList
│   ├── SessionCard
│   └── SessionFilters
├── SessionDetail
│   ├── TranscriptViewer
│   ├── ToolOutputsViewer
│   └── AttachmentGallery
└── ArchiveView
    └── ArchivedSessionList
```

---

## 3. API Design

### 3.1 RPC Methods (Gateway)

**New methods needed in `src/gateway/handlers/memory.rs`** (or new module):

```rust
// List sessions with pagination and filters
#[derive(Deserialize)]
pub struct ListSessionsRequest {
    pub agent_id: Option<String>,
    pub workspace_id: Option<String>,
    pub start_date: Option<i64>,      // Unix timestamp
    pub end_date: Option<i64>,
    pub tags: Option<Vec<String>>,
    pub compression_status: Option<String>, // "pending", "compressed", "archived"
    pub limit: usize,
    pub offset: usize,
}

#[derive(Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub agent_id: String,
    pub workspace_id: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub duration_seconds: Option<i64>,
    pub message_count: i64,
    pub tool_call_count: i64,
    pub tags: Vec<String>,
    pub compression_status: String,
}

// Get full session details
#[derive(Deserialize)]
pub struct GetSessionRequest {
    pub session_id: String,
}

#[derive(Serialize)]
pub struct SessionDetail {
    pub metadata: SessionSummary,
    pub transcript_markdown: String,      // Full transcript.md content
    pub tool_outputs_markdown: String,    // Full tool_outputs.md content
    pub attachments: Vec<AttachmentInfo>,
}

#[derive(Serialize)]
pub struct AttachmentInfo {
    pub attachment_id: String,
    pub file_path: String,                // Relative path
    pub content_hash: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub uploaded_at: i64,
}

// Search sessions
#[derive(Deserialize)]
pub struct SearchSessionsRequest {
    pub query: String,
    pub agent_id: Option<String>,
    pub limit: usize,
}

#[derive(Serialize)]
pub struct SessionSearchResult {
    pub session_id: String,
    pub agent_id: String,
    pub started_at: i64,
    pub snippet: String,                  // Matched text excerpt
    pub score: f32,
}

// Get attachment content (for display)
#[derive(Deserialize)]
pub struct GetAttachmentRequest {
    pub session_id: String,
    pub attachment_id: String,
}

// Returns binary data with mime_type header
```

### 3.2 TypeScript API Client

**New file**: `interfaces/webchat/src/api/sessions.ts`

```typescript
export interface SessionSummary {
  session_id: string;
  agent_id: string;
  workspace_id?: string;
  started_at: number;
  ended_at?: number;
  duration_seconds?: number;
  message_count: number;
  tool_call_count: number;
  tags: string[];
  compression_status: string;
}

export interface SessionDetail {
  metadata: SessionSummary;
  transcript_markdown: string;
  tool_outputs_markdown: string;
  attachments: AttachmentInfo[];
}

export interface AttachmentInfo {
  attachment_id: string;
  file_path: string;
  content_hash: string;
  mime_type: string;
  size_bytes: number;
  uploaded_at: number;
}

export class SessionsApi {
  static async list(
    state: DashboardState,
    filters: {
      agent_id?: string;
      workspace_id?: string;
      start_date?: number;
      end_date?: number;
      tags?: string[];
      compression_status?: string;
      limit: number;
      offset: number;
    }
  ): Promise<SessionSummary[]> {
    const resp = await state.rpc_call("memory.sessions.list", filters);
    return resp.sessions;
  }

  static async get(
    state: DashboardState,
    session_id: string
  ): Promise<SessionDetail> {
    return await state.rpc_call("memory.sessions.get", { session_id });
  }

  static async search(
    state: DashboardState,
    query: string,
    agent_id?: string,
    limit: number = 20
  ): Promise<SessionSearchResult[]> {
    const resp = await state.rpc_call("memory.sessions.search", {
      query,
      agent_id,
      limit,
    });
    return resp.results;
  }

  static getAttachmentUrl(
    session_id: string,
    attachment_id: string
  ): string {
    // Construct URL to fetch attachment via gateway
    return `/api/memory/sessions/${session_id}/attachments/${attachment_id}`;
  }
}
```

---

## 4. UI Components

### 4.1 SessionsView (Main Container)

**File**: `interfaces/webchat/src/platform/wide/views/settings/sessions.rs`

```rust
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{SessionsApi, SessionSummary};
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};

#[component]
pub fn SessionsView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let sessions = RwSignal::new(Vec::<SessionSummary>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let selected_session = RwSignal::new(Option::<String>::None);

    // Filters
    let agent_filter = RwSignal::new(Option::<String>::None);
    let status_filter = RwSignal::new(Option::<String>::None);
    let page = RwSignal::new(0_usize);
    let page_size = 20;

    // Load sessions
    let load_sessions = move || {
        spawn_local(async move {
            loading.set(true);
            match SessionsApi::list(
                &state,
                agent_filter.get(),
                status_filter.get(),
                page.get() * page_size,
                page_size,
            ).await {
                Ok(list) => {
                    sessions.set(list);
                    error.set(None);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to load sessions: {}", e)));
                }
            }
            loading.set(false);
        });
    };

    Effect::new(move || {
        if state.is_connected.get() {
            load_sessions();
        }
    });

    view! {
        <div class="flex h-full">
            // Left sidebar: Session list
            <div class="w-1/3 border-r border-border overflow-y-auto">
                <div class="p-4">
                    <h1 class="text-xl font-bold mb-4">{t!(i18n, settings.memory.sessions)}</h1>

                    <SessionFilters
                        agent_filter=agent_filter
                        status_filter=status_filter
                        on_change=load_sessions
                    />

                    {move || {
                        if loading.get() {
                            view! { <div class="text-text-tertiary">{t!(i18n, common.loading)}</div> }.into_any()
                        } else if let Some(err) = error.get() {
                            view! { <div class="text-danger">{err}</div> }.into_any()
                        } else {
                            view! {
                                <div class="space-y-2">
                                    {sessions.get().into_iter().map(|session| {
                                        view! {
                                            <SessionCard
                                                session=session
                                                selected=selected_session
                                            />
                                        }
                                    }).collect_view()}
                                </div>
                            }.into_any()
                        }
                    }}

                    <Pagination page=page total={move || sessions.get().len()} page_size=page_size />
                </div>
            </div>

            // Right panel: Session detail
            <div class="flex-1 overflow-y-auto">
                {move || {
                    if let Some(session_id) = selected_session.get() {
                        view! { <SessionDetail session_id=session_id /> }.into_any()
                    } else {
                        view! {
                            <div class="flex items-center justify-center h-full text-text-tertiary">
                                {t!(i18n, settings.memory.select_session)}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}
```

### 4.2 SessionCard (List Item)

```rust
#[component]
fn SessionCard(
    session: SessionSummary,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let session_id = session.session_id.clone();

    let is_selected = move || {
        selected.get().as_ref() == Some(&session_id)
    };

    let formatted_date = format_timestamp(session.started_at);
    let duration = session.duration_seconds
        .map(|d| format!("{}m", d / 60))
        .unwrap_or_else(|| "—".to_string());

    view! {
        <div
            on:click=move |_| selected.set(Some(session_id.clone()))
            class=move || {
                let base = "p-3 rounded cursor-pointer hover:bg-surface-raised border";
                if is_selected() {
                    format!("{} border-info bg-surface-raised", base)
                } else {
                    format!("{} border-transparent", base)
                }
            }
        >
            <div class="flex justify-between items-start mb-1">
                <span class="font-medium text-sm">{session.agent_id.clone()}</span>
                <span class="text-xs text-text-tertiary">{duration}</span>
            </div>

            <div class="text-xs text-text-secondary mb-2">{formatted_date}</div>

            <div class="flex flex-wrap gap-1">
                {session.tags.iter().map(|tag| {
                    view! {
                        <span class="px-2 py-0.5 bg-surface-sunken text-xs rounded">
                            {tag.clone()}
                        </span>
                    }
                }).collect_view()}
            </div>

            <div class="flex justify-between items-center mt-2 text-xs">
                <span class="text-text-tertiary">
                    {format!("{} msg · {} tools", session.message_count, session.tool_call_count)}
                </span>
                <CompressionBadge status=session.compression_status.clone() />
            </div>
        </div>
    }
}

#[component]
fn CompressionBadge(status: String) -> impl IntoView {
    let i18n = use_i18n();
    let (label, color) = match status.as_str() {
        "pending" => (t!(i18n, settings.memory.status_pending), "text-warning"),
        "compressed" => (t!(i18n, settings.memory.status_compressed), "text-success"),
        "archived" => (t!(i18n, settings.memory.status_archived), "text-text-tertiary"),
        _ => (status.clone(), "text-text-secondary"),
    };

    view! {
        <span class=format!("text-xs {}", color)>{label}</span>
    }
}
```

### 4.3 SessionDetail (Right Panel)

```rust
#[component]
fn SessionDetail(session_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let detail = RwSignal::new(Option::<SessionDetail>::None);
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let active_tab = RwSignal::new("transcript");

    Effect::new(move || {
        let sid = session_id.clone();
        spawn_local(async move {
            loading.set(true);
            match SessionsApi::get(&state, &sid).await {
                Ok(d) => {
                    detail.set(Some(d));
                    error.set(None);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to load session: {}", e)));
                }
            }
            loading.set(false);
        });
    });

    view! {
        <div class="p-6">
            {move || {
                if loading.get() {
                    view! { <div class="text-text-tertiary">{t!(i18n, common.loading)}</div> }.into_any()
                } else if let Some(err) = error.get() {
                    view! { <div class="text-danger">{err}</div> }.into_any()
                } else if let Some(d) = detail.get() {
                    view! {
                        <div class="space-y-4">
                            // Metadata header
                            <div class="border-b border-border pb-4">
                                <h2 class="text-xl font-bold mb-2">{d.metadata.agent_id.clone()}</h2>
                                <div class="text-sm text-text-secondary space-y-1">
                                    <div>{format!("Started: {}", format_timestamp(d.metadata.started_at))}</div>
                                    {d.metadata.ended_at.map(|t| view! {
                                        <div>{format!("Ended: {}", format_timestamp(t))}</div>
                                    })}
                                    {d.metadata.workspace_id.as_ref().map(|ws| view! {
                                        <div>{format!("Workspace: {}", ws)}</div>
                                    })}
                                </div>
                            </div>

                            // Tabs
                            <div class="flex gap-4 border-b border-border">
                                <TabButton
                                    label={t!(i18n, settings.memory.transcript)}
                                    tab_id="transcript"
                                    active_tab=active_tab
                                />
                                <TabButton
                                    label={t!(i18n, settings.memory.tool_outputs)}
                                    tab_id="tools"
                                    active_tab=active_tab
                                />
                                <TabButton
                                    label={format!("{} ({})", t!(i18n, settings.memory.attachments), d.attachments.len())}
                                    tab_id="attachments"
                                    active_tab=active_tab
                                />
                            </div>

                            // Tab content
                            {move || {
                                match active_tab.get().as_str() {
                                    "transcript" => view! {
                                        <TranscriptViewer markdown=d.transcript_markdown.clone() />
                                    }.into_any(),
                                    "tools" => view! {
                                        <ToolOutputsViewer markdown=d.tool_outputs_markdown.clone() />
                                    }.into_any(),
                                    "attachments" => view! {
                                        <AttachmentGallery
                                            session_id=session_id.clone()
                                            attachments=d.attachments.clone()
                                        />
                                    }.into_any(),
                                    _ => view! { <div></div> }.into_any(),
                                }
                            }}
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn TabButton(
    label: String,
    tab_id: &'static str,
    active_tab: RwSignal<&'static str>,
) -> impl IntoView {
    let is_active = move || active_tab.get() == tab_id;

    view! {
        <button
            on:click=move |_| active_tab.set(tab_id)
            class=move || {
                let base = "px-4 py-2 font-medium";
                if is_active() {
                    format!("{} border-b-2 border-info text-info", base)
                } else {
                    format!("{} text-text-secondary hover:text-text-primary", base)
                }
            }
        >
            {label}
        </button>
    }
}
```

### 4.4 TranscriptViewer (Markdown Renderer)

```rust
#[component]
fn TranscriptViewer(markdown: String) -> impl IntoView {
    let i18n = use_i18n();

    // Parse Markdown to HTML (using pulldown-cmark or similar)
    let html = render_markdown(&markdown);

    view! {
        <div class="prose prose-slate max-w-none">
            <div inner_html=html></div>
        </div>
    }
}

// Helper function to render Markdown to HTML
fn render_markdown(md: &str) -> String {
    use pulldown_cmark::{Parser, Options, html};
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(md, options);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}
```

### 4.5 AttachmentGallery

```rust
#[component]
fn AttachmentGallery(
    session_id: String,
    attachments: Vec<AttachmentInfo>,
) -> impl IntoView {
    let i18n = use_i18n();

    if attachments.is_empty() {
        return view! {
            <div class="text-text-tertiary text-center py-8">
                {t!(i18n, settings.memory.no_attachments)}
            </div>
        }.into_any();
    }

    view! {
        <div class="grid grid-cols-3 gap-4">
            {attachments.into_iter().map(|att| {
                let url = SessionsApi::get_attachment_url(&session_id, &att.attachment_id);
                view! {
                    <AttachmentCard attachment=att url=url />
                }
            }).collect_view()}
        </div>
    }.into_any()
}

#[component]
fn AttachmentCard(attachment: AttachmentInfo, url: String) -> impl IntoView {
    let is_image = attachment.mime_type.starts_with("image/");
    let size_display = format_bytes(attachment.size_bytes);

    view! {
        <div class="border border-border rounded overflow-hidden hover:border-info transition-colors">
            {if is_image {
                view! {
                    <img src=url.clone() alt="Attachment" class="w-full h-40 object-cover" />
                }.into_any()
            } else {
                view! {
                    <div class="w-full h-40 bg-surface-sunken flex items-center justify-center">
                        <FileIcon mime_type=attachment.mime_type.clone() />
                    </div>
                }.into_any()
            }}

            <div class="p-2">
                <div class="text-xs text-text-secondary truncate">{attachment.attachment_id}</div>
                <div class="text-xs text-text-tertiary">{size_display}</div>
                <a
                    href=url
                    target="_blank"
                    class="text-xs text-info hover:underline"
                >
                    {t!(i18n, common.view)}
                </a>
            </div>
        </div>
    }
}

#[component]
fn FileIcon(mime_type: String) -> impl IntoView {
    let icon = if mime_type.starts_with("video/") {
        "🎥"
    } else if mime_type.starts_with("audio/") {
        "🎵"
    } else if mime_type.contains("pdf") {
        "📄"
    } else {
        "📎"
    };

    view! {
        <span class="text-4xl">{icon}</span>
    }
}

fn format_bytes(bytes: i64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
```

### 4.6 SessionFilters

```rust
#[component]
fn SessionFilters(
    agent_filter: RwSignal<Option<String>>,
    status_filter: RwSignal<Option<String>>,
    on_change: impl Fn() + 'static,
) -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <div class="space-y-3 mb-4">
            // Agent filter
            <div>
                <label class="block text-xs font-medium mb-1">{t!(i18n, settings.memory.filter_agent)}</label>
                <select
                    on:change=move |ev| {
                        let val = event_target_value(&ev);
                        agent_filter.set(if val.is_empty() { None } else { Some(val) });
                        on_change();
                    }
                    class="w-full px-2 py-1 text-sm border border-border rounded bg-surface-raised"
                >
                    <option value="">{t!(i18n, common.all)}</option>
                    <option value="main">"main"</option>
                    <option value="assistant">"assistant"</option>
                </select>
            </div>

            // Status filter
            <div>
                <label class="block text-xs font-medium mb-1">{t!(i18n, settings.memory.filter_status)}</label>
                <select
                    on:change=move |ev| {
                        let val = event_target_value(&ev);
                        status_filter.set(if val.is_empty() { None } else { Some(val) });
                        on_change();
                    }
                    class="w-full px-2 py-1 text-sm border border-border rounded bg-surface-raised"
                >
                    <option value="">{t!(i18n, common.all)}</option>
                    <option value="pending">{t!(i18n, settings.memory.status_pending)}</option>
                    <option value="compressed">{t!(i18n, settings.memory.status_compressed)}</option>
                    <option value="archived">{t!(i18n, settings.memory.status_archived)}</option>
                </select>
            </div>
        </div>
    }
}
```

---

## 5. Gateway Implementation

### 5.1 Handler Registration

**File**: `src/gateway/handlers/memory_sessions.rs` (NEW)

```rust
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::memory::store::RawSessionStore;

pub struct MemorySessionsHandler {
    session_store: Arc<dyn RawSessionStore>,
}

impl MemorySessionsHandler {
    pub fn new(session_store: Arc<dyn RawSessionStore>) -> Self {
        Self { session_store }
    }

    pub async fn list_sessions(
        State(handler): State<Arc<Self>>,
        Query(req): Query<ListSessionsRequest>,
    ) -> Result<Json<ListSessionsResponse>, AppError> {
        let sessions = handler.session_store
            .list_sessions(
                req.agent_id.as_deref(),
                req.workspace_id.as_deref(),
                req.start_date,
                req.end_date,
                req.compression_status.as_deref(),
                req.limit,
                req.offset,
            )
            .await?;

        Ok(Json(ListSessionsResponse { sessions }))
    }

    pub async fn get_session(
        State(handler): State<Arc<Self>>,
        Path(session_id): Path<String>,
    ) -> Result<Json<SessionDetail>, AppError> {
        let metadata = handler.session_store.get_session_metadata(&session_id).await?;
        let transcript = handler.session_store.read_transcript(&session_id).await?;
        let tool_outputs = handler.session_store.read_tool_outputs(&session_id).await?;
        let attachments = handler.session_store.list_attachments(&session_id).await?;

        Ok(Json(SessionDetail {
            metadata,
            transcript_markdown: transcript,
            tool_outputs_markdown: tool_outputs,
            attachments,
        }))
    }

    pub async fn search_sessions(
        State(handler): State<Arc<Self>>,
        Query(req): Query<SearchSessionsRequest>,
    ) -> Result<Json<SearchSessionsResponse>, AppError> {
        // Use FTS5 index to search transcripts
        let results = handler.session_store
            .search_transcripts(&req.query, req.agent_id.as_deref(), req.limit)
            .await?;

        Ok(Json(SearchSessionsResponse { results }))
    }

    pub async fn get_attachment(
        State(handler): State<Arc<Self>>,
        Path((session_id, attachment_id)): Path<(String, String)>,
    ) -> Result<Response, AppError> {
        let attachment = handler.session_store
            .get_attachment(&session_id, &attachment_id)
            .await?;

        // Return binary data with appropriate Content-Type
        Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, attachment.mime_type)],
            attachment.data,
        ).into_response())
    }
}

#[derive(Deserialize)]
pub struct ListSessionsRequest {
    pub agent_id: Option<String>,
    pub workspace_id: Option<String>,
    pub start_date: Option<i64>,
    pub end_date: Option<i64>,
    pub compression_status: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Serialize)]
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionSummary>,
}

// ... other request/response structs
```

### 5.2 Route Registration

**File**: `src/gateway/router.rs`

```rust
use axum::routing::{get, post};

pub fn memory_sessions_routes(handler: Arc<MemorySessionsHandler>) -> Router {
    Router::new()
        .route("/api/memory/sessions", get(MemorySessionsHandler::list_sessions))
        .route("/api/memory/sessions/:id", get(MemorySessionsHandler::get_session))
        .route("/api/memory/sessions/search", get(MemorySessionsHandler::search_sessions))
        .route(
            "/api/memory/sessions/:session_id/attachments/:attachment_id",
            get(MemorySessionsHandler::get_attachment),
        )
        .with_state(handler)
}
```

---

## 6. Search Implementation

### 6.1 FTS5 Index Schema

**File**: `src/memory/store/sqlite/schema/ddl.rs` (extend existing)

```sql
-- Full-text search on session transcripts
CREATE VIRTUAL TABLE session_transcripts_fts USING fts5(
    session_id UNINDEXED,
    agent_id UNINDEXED,
    content,
    tokenize = 'porter unicode61'
);

-- Trigger to auto-update FTS5 on session changes
CREATE TRIGGER session_transcript_update_fts
AFTER UPDATE OF content_hash ON sessions
BEGIN
    DELETE FROM session_transcripts_fts WHERE session_id = OLD.session_id;
    INSERT INTO session_transcripts_fts (session_id, agent_id, content)
    VALUES (
        NEW.session_id,
        NEW.agent_id,
        (SELECT content FROM read_file(NEW.transcript_path))
    );
END;
```

### 6.2 Search Query

```rust
impl MarkdownSessionStore {
    pub async fn search_transcripts(
        &self,
        query: &str,
        agent_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SessionSearchResult>, AlephError> {
        let conn = self.conn.lock().await;

        let mut sql = String::from(
            "SELECT session_id, agent_id, snippet(session_transcripts_fts, 2, '<mark>', '</mark>', '...', 64) as snippet, rank
             FROM session_transcripts_fts
             WHERE session_transcripts_fts MATCH ?"
        );

        if agent_id.is_some() {
            sql.push_str(" AND agent_id = ?");
        }

        sql.push_str(" ORDER BY rank LIMIT ?");

        let mut stmt = conn.prepare(&sql)?;

        let results = if let Some(aid) = agent_id {
            stmt.query_map(params![query, aid, limit], |row| {
                Ok(SessionSearchResult {
                    session_id: row.get(0)?,
                    agent_id: row.get(1)?,
                    snippet: row.get(2)?,
                    score: row.get::<_, f32>(3)?,
                    started_at: 0, // TODO: JOIN with sessions table
                })
            })?
        } else {
            stmt.query_map(params![query, limit], |row| {
                Ok(SessionSearchResult {
                    session_id: row.get(0)?,
                    agent_id: row.get(1)?,
                    snippet: row.get(2)?,
                    score: row.get::<_, f32>(3)?,
                    started_at: 0,
                })
            })?
        };

        results.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}
```

---

## 7. Internationalization

### 7.1 New i18n Keys

**File**: `interfaces/webchat/locales/en.json` (extend)

```json
{
  "settings": {
    "memory": {
      "sessions": "Sessions",
      "transcript": "Transcript",
      "tool_outputs": "Tool Outputs",
      "attachments": "Attachments",
      "select_session": "Select a session to view details",
      "no_attachments": "No attachments",
      "filter_agent": "Filter by Agent",
      "filter_status": "Filter by Status",
      "status_pending": "Pending",
      "status_compressed": "Compressed",
      "status_archived": "Archived",
      "search_placeholder": "Search sessions..."
    }
  }
}
```

**File**: `interfaces/webchat/locales/zh-CN.json`

```json
{
  "settings": {
    "memory": {
      "sessions": "会话",
      "transcript": "对话记录",
      "tool_outputs": "工具输出",
      "attachments": "附件",
      "select_session": "选择一个会话查看详情",
      "no_attachments": "无附件",
      "filter_agent": "按代理筛选",
      "filter_status": "按状态筛选",
      "status_pending": "待压缩",
      "status_compressed": "已压缩",
      "status_archived": "已归档",
      "search_placeholder": "搜索会话..."
    }
  }
}
```

---

## 8. Performance Considerations

### 8.1 Lazy Loading

- **Session list**: Paginated (20 per page)
- **Transcript**: Load only when tab is active
- **Attachments**: Thumbnail images lazy-loaded

### 8.2 Caching

- Cache session metadata in frontend (5 minutes TTL)
- Gateway caches recent transcript reads in memory (LRU, max 10 sessions)

### 8.3 Large Transcripts

- For transcripts > 100KB, add "Load More" pagination
- Display first 50 messages by default

---

## 9. Implementation Plan

### Phase A: Backend (3-4 days)

1. Implement `RawSessionStore` trait methods for listing/searching
2. Add `session_transcripts_fts` table and triggers
3. Create `MemorySessionsHandler` with RPC endpoints
4. Register routes in gateway router
5. Unit tests for search/filter

### Phase B: Frontend Components (4-5 days)

1. Create `sessions.rs` view and components
2. Implement `SessionsApi` TypeScript client
3. Build SessionCard, SessionFilters, Pagination
4. Add TranscriptViewer with Markdown rendering
5. Implement AttachmentGallery with image previews

### Phase C: Integration (2-3 days)

1. Add "Sessions" menu item in settings
2. Wire up navigation routing
3. Test with real session data
4. Add i18n strings
5. Polish UI/UX

### Phase D: Testing (2 days)

1. Unit tests for API client
2. Integration tests for RPC endpoints
3. Manual testing: create sessions, view transcripts, search
4. Performance testing with 1000+ sessions

**Total**: 11-14 days

---

## 10. Acceptance Criteria

- ✅ List sessions with filters (agent, status, date range)
- ✅ View full transcript in Markdown format
- ✅ View tool outputs separately
- ✅ Display attachments with previews
- ✅ Search across all transcripts (FTS5)
- ✅ Pagination for large session lists
- ✅ Responsive UI (works on desktop and tablet)
- ✅ Internationalized (EN + ZH-CN)
- ✅ Performance: < 100ms for session list, < 500ms for transcript load

---

## 11. Future Enhancements (Deferred)

1. **Session annotations** — add user notes to sessions
2. **Export sessions** — download as Markdown/PDF
3. **Session comparison** — diff two transcripts
4. **Timeline view** — visualize sessions on calendar
5. **Starred sessions** — bookmark important sessions
6. **Delete sessions** — manual cleanup (with confirmation)

---

**Design Status**: Ready for Implementation
**Next Step**: Begin Phase A — Backend RPC Endpoints
