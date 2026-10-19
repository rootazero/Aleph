# Phase D: Panel UI Implementation - Detailed Plan

**Date**: 2026-10-19  
**Status**: Planning  
**Prerequisites**: Phase A + B + C complete

---

## Overview

Build a comprehensive session browser UI in the Aleph Panel (Leptos/WASM) that allows users to:
- Browse historical sessions
- View full transcripts with Markdown rendering
- Search across sessions using FTS5
- Download attachments
- Filter by agent, date, status

---

## Architecture

### Frontend (Leptos/WASM)
```
interfaces/webchat/src/platform/wide/views/settings/
├── memory.rs              (existing - config UI)
└── sessions/              (NEW)
    ├── mod.rs             - Route registration
    ├── view.rs            - Main SessionsView component
    ├── list.rs            - SessionList + SessionCard
    ├── detail.rs          - SessionDetail (3 tabs)
    ├── transcript.rs      - TranscriptViewer (Markdown renderer)
    ├── tools.rs           - ToolOutputsTab
    ├── attachments.rs     - AttachmentGallery
    ├── filters.rs         - SessionFilters component
    └── search.rs          - SearchBar component
```

### Backend (RPC Handlers)
```
src/gateway/handlers/
└── memory/
    ├── mod.rs             (existing)
    └── sessions.rs        (NEW)
        - list_sessions()
        - get_session()
        - search_sessions()
        - download_attachment()
```

### Data Flow
```
Panel UI (Leptos)
    ↓ RPC call
Gateway Handler
    ↓ query
MarkdownSessionStore
    ↓ read
~/.aleph/raw/sessions/
```

---

## Component Specifications

### 1. SessionsView (Main Container)

**File**: `interfaces/webchat/src/platform/wide/views/settings/sessions/view.rs`

**Layout**: Split view (60/40)
```
┌─────────────────────────────────────────────────┐
│ Sessions Browser                                │
├─────────────┬───────────────────────────────────┤
│             │                                   │
│  Session    │  Session Detail                   │
│  List       │  ┌─────────────────────────────┐ │
│  (left)     │  │ Transcript │ Tools │ Files  │ │
│             │  └─────────────────────────────┘ │
│  ┌────────┐│                                   │
│  │ Card 1 ││  [Markdown content rendered]      │
│  ├────────┤│                                   │
│  │ Card 2 ││                                   │
│  ├────────┤│                                   │
│  │ Card 3 ││                                   │
│  └────────┘│                                   │
│             │                                   │
└─────────────┴───────────────────────────────────┘
```

**Props**: None (uses route params)

**State**:
```rust
struct SessionsViewState {
    sessions: Vec<SessionMetadata>,
    selected_session: Option<String>,
    loading: bool,
    error: Option<String>,
    filters: SessionFilters,
    search_query: String,
}
```

**Component**:
```rust
#[component]
pub fn SessionsView() -> impl IntoView {
    let (sessions, set_sessions) = create_signal(Vec::new());
    let (selected, set_selected) = create_signal(None);
    
    // Load sessions on mount
    create_effect(move |_| {
        spawn_local(async move {
            let result = rpc::memory_sessions_list(filters).await;
            set_sessions.set(result);
        });
    });
    
    view! {
        <div class="sessions-view">
            <SessionFilters on_change={/* reload sessions */} />
            <div class="sessions-split">
                <SessionList
                    sessions={sessions}
                    selected={selected}
                    on_select={set_selected}
                />
                <SessionDetail session_id={selected} />
            </div>
        </div>
    }
}
```

**Estimated**: ~200 lines

---

### 2. SessionList + SessionCard

**File**: `interfaces/webchat/src/platform/wide/views/settings/sessions/list.rs`

**SessionCard Layout**:
```
┌───────────────────────────────────────┐
│ 🤖 agent-name                  [tag] │
│ Started: 2026-10-19 14:32           │
│ Duration: 45m 23s                    │
│ 127 messages • 8 tool calls          │
│ "Last message preview text..."       │
└───────────────────────────────────────┘
```

**Component**:
```rust
#[component]
pub fn SessionCard(
    metadata: SessionMetadata,
    selected: bool,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        <div
            class={format!("session-card {}", if selected { "selected" } else { "" })}
            on:click=move |_| on_click()
        >
            <div class="session-header">
                <span class="agent-icon">🤖</span>
                <span class="agent-name">{metadata.agent_id}</span>
                {metadata.tags.iter().map(|tag| {
                    view! { <span class="tag">{tag}</span> }
                }).collect_view()}
            </div>
            
            <div class="session-meta">
                <span class="timestamp">
                    {format_timestamp(metadata.started_at)}
                </span>
                <span class="duration">
                    {format_duration(metadata.duration_seconds)}
                </span>
            </div>
            
            <div class="session-stats">
                <span>{metadata.message_count} messages</span>
                <span> • </span>
                <span>{metadata.tool_call_count} tool calls</span>
            </div>
            
            <div class="preview">
                {get_preview_text(&metadata)}
            </div>
        </div>
    }
}
```

**Pagination**:
```rust
#[component]
pub fn SessionList(
    sessions: Signal<Vec<SessionMetadata>>,
    selected: Signal<Option<String>>,
    on_select: impl Fn(String) + 'static,
) -> impl IntoView {
    let (page, set_page) = create_signal(0);
    let page_size = 20;
    
    let paginated = move || {
        let start = page() * page_size;
        sessions().into_iter().skip(start).take(page_size).collect::<Vec<_>>()
    };
    
    view! {
        <div class="session-list">
            <For
                each=paginated
                key=|meta| meta.session_id.clone()
                children=move |meta| {
                    let session_id = meta.session_id.clone();
                    let is_selected = move || selected().as_ref() == Some(&session_id);
                    view! {
                        <SessionCard
                            metadata={meta.clone()}
                            selected={is_selected()}
                            on_click={move || on_select(session_id.clone())}
                        />
                    }
                }
            />
            <Pagination page={page} total={sessions().len() / page_size} on_change={set_page} />
        </div>
    }
}
```

**Estimated**: ~250 lines

---

### 3. SessionDetail (3 Tabs)

**File**: `interfaces/webchat/src/platform/wide/views/settings/sessions/detail.rs`

**Tabs**:
1. **Transcript** - Full conversation history (Markdown)
2. **Tool Outputs** - Tool execution logs
3. **Attachments** - Files (images, videos, documents)

**Component**:
```rust
#[component]
pub fn SessionDetail(
    session_id: Signal<Option<String>>,
) -> impl IntoView {
    let (active_tab, set_active_tab) = create_signal(Tab::Transcript);
    let (session_data, set_session_data) = create_signal(None);
    
    // Load session data when ID changes
    create_effect(move |_| {
        if let Some(id) = session_id() {
            spawn_local(async move {
                let data = rpc::memory_sessions_get(&id).await;
                set_session_data.set(Some(data));
            });
        }
    });
    
    view! {
        <div class="session-detail">
            {move || session_id().map(|_| view! {
                <div class="tabs">
                    <button
                        class={move || if active_tab() == Tab::Transcript { "active" } else { "" }}
                        on:click=move |_| set_active_tab(Tab::Transcript)
                    >
                        "Transcript"
                    </button>
                    <button
                        class={move || if active_tab() == Tab::Tools { "active" } else { "" }}
                        on:click=move |_| set_active_tab(Tab::Tools)
                    >
                        "Tool Outputs"
                    </button>
                    <button
                        class={move || if active_tab() == Tab::Attachments { "active" } else { "" }}
                        on:click=move |_| set_active_tab(Tab::Attachments)
                    >
                        "Attachments"
                    </button>
                </div>
                
                <div class="tab-content">
                    {move || match active_tab() {
                        Tab::Transcript => view! {
                            <TranscriptViewer session_data={session_data} />
                        }.into_view(),
                        Tab::Tools => view! {
                            <ToolOutputsTab session_data={session_data} />
                        }.into_view(),
                        Tab::Attachments => view! {
                            <AttachmentGallery session_data={session_data} />
                        }.into_view(),
                    }}
                </div>
            })}
        </div>
    }
}
```

**Estimated**: ~150 lines

---

### 4. TranscriptViewer (Markdown Renderer)

**File**: `interfaces/webchat/src/platform/wide/views/settings/sessions/transcript.rs`

**Dependencies**:
- `pulldown-cmark` for Markdown parsing
- Custom CSS for message styling

**Component**:
```rust
use pulldown_cmark::{Parser, Options, html};

#[component]
pub fn TranscriptViewer(
    session_data: Signal<Option<SessionData>>,
) -> impl IntoView {
    let rendered_html = move || {
        session_data()
            .and_then(|data| data.transcript)
            .map(|markdown| {
                let parser = Parser::new_ext(&markdown, Options::all());
                let mut html_output = String::new();
                html::push_html(&mut html_output, parser);
                html_output
            })
            .unwrap_or_default()
    };
    
    view! {
        <div class="transcript-viewer">
            <div class="transcript-content"
                 inner_html={rendered_html}
            />
        </div>
    }
}
```

**CSS Styling**:
```css
.transcript-viewer {
    padding: 20px;
    max-width: 800px;
}

.transcript-content h2 {
    /* Timestamp headers */
    color: #666;
    font-size: 0.9em;
    margin-top: 20px;
}

.transcript-content h3 {
    /* Role (User/Assistant/System) */
    font-weight: 600;
    margin-bottom: 10px;
}

.transcript-content p {
    line-height: 1.6;
    margin-bottom: 15px;
}

.transcript-content code {
    background: #f5f5f5;
    padding: 2px 6px;
    border-radius: 3px;
}

.transcript-content pre {
    background: #f5f5f5;
    padding: 15px;
    border-radius: 5px;
    overflow-x: auto;
}
```

**Estimated**: ~100 lines

---

### 5. ToolOutputsTab

**File**: `interfaces/webchat/src/platform/wide/views/settings/sessions/tools.rs`

**Layout**:
```
┌────────────────────────────────────────┐
│ Tool: bash                             │
│ Args: ls -la /tmp                      │
│ ────────────────────────────────────── │
│ stdout:                                │
│   drwxr-xr-x  10 user  wheel  ...     │
│   -rw-r--r--   1 user  wheel  ...     │
│ ────────────────────────────────────── │
│ Exit Code: 0 • Duration: 234ms         │
└────────────────────────────────────────┘
```

**Component**:
```rust
#[component]
pub fn ToolOutputsTab(
    session_data: Signal<Option<SessionData>>,
) -> impl IntoView {
    let tool_outputs = move || {
        session_data()
            .and_then(|data| data.tool_outputs)
            .unwrap_or_default()
    };
    
    view! {
        <div class="tool-outputs-tab">
            <For
                each=move || parse_tool_outputs(&tool_outputs())
                key=|output| output.index
                children=move |output| view! {
                    <div class="tool-output-card">
                        <div class="tool-header">
                            <span class="tool-name">Tool: {output.tool_name}</span>
                            <span class="timestamp">{output.timestamp}</span>
                        </div>
                        
                        {(!output.args.is_empty()).then(|| view! {
                            <div class="tool-args">
                                <strong>Args:</strong>
                                <pre>{output.args}</pre>
                            </div>
                        })}
                        
                        {(!output.stdout.is_empty()).then(|| view! {
                            <div class="tool-stdout">
                                <strong>stdout:</strong>
                                <pre>{output.stdout}</pre>
                            </div>
                        })}
                        
                        {(!output.stderr.is_empty()).then(|| view! {
                            <div class="tool-stderr error">
                                <strong>stderr:</strong>
                                <pre>{output.stderr}</pre>
                            </div>
                        })}
                        
                        <div class="tool-footer">
                            <span>Exit Code: {output.exit_code.unwrap_or(-1)}</span>
                            {output.duration_ms.map(|ms| view! {
                                <span> • Duration: {ms}ms</span>
                            })}
                        </div>
                    </div>
                }
            />
        </div>
    }
}
```

**Estimated**: ~150 lines

---

### 6. AttachmentGallery

**File**: `interfaces/webchat/src/platform/wide/views/settings/sessions/attachments.rs`

**Layout**:
```
┌─────────┬─────────┬─────────┐
│  IMG1   │  IMG2   │  PDF1   │
│ 1.2 MB  │ 850 KB  │ 5.4 MB  │
│[Download│[Download│[Download│
└─────────┴─────────┴─────────┘
```

**Component**:
```rust
#[component]
pub fn AttachmentGallery(
    session_data: Signal<Option<SessionData>>,
) -> impl IntoView {
    let attachments = move || {
        session_data()
            .and_then(|data| data.attachments)
            .unwrap_or_default()
    };
    
    view! {
        <div class="attachment-gallery">
            <For
                each=attachments
                key=|att| att.attachment_id.clone()
                children=move |att| view! {
                    <div class="attachment-card">
                        {render_preview(&att)}
                        
                        <div class="attachment-meta">
                            <span class="file-name">{get_file_name(&att)}</span>
                            <span class="file-size">{format_size(att.size_bytes)}</span>
                        </div>
                        
                        <button
                            class="download-btn"
                            on:click=move |_| download_attachment(&att.attachment_id)
                        >
                            "Download"
                        </button>
                    </div>
                }
            />
        </div>
    }
}

fn render_preview(att: &AttachmentRef) -> View {
    if att.mime_type.starts_with("image/") {
        view! {
            <img
                src={format!("/api/memory/sessions/attachment/{}", att.attachment_id)}
                alt="Attachment preview"
                class="attachment-preview-image"
            />
        }.into_view()
    } else {
        view! {
            <div class="attachment-preview-icon">
                {get_icon_for_mime(&att.mime_type)}
            </div>
        }.into_view()
    }
}

async fn download_attachment(attachment_id: &str) {
    // Trigger browser download
    let url = format!("/api/memory/sessions/attachment/{}", attachment_id);
    web_sys::window()
        .unwrap()
        .location()
        .set_href(&url)
        .unwrap();
}
```

**Estimated**: ~200 lines

---

### 7. SessionFilters

**File**: `interfaces/webchat/src/platform/wide/views/settings/sessions/filters.rs`

**UI**:
```
[Agent: All ▼] [Status: All ▼] [Date: Last 30 days ▼] [Apply]
```

**Component**:
```rust
#[component]
pub fn SessionFilters(
    on_change: impl Fn(Filters) + 'static,
) -> impl IntoView {
    let (agent_filter, set_agent_filter) = create_signal(None::<String>);
    let (status_filter, set_status_filter) = create_signal(None::<String>);
    let (date_filter, set_date_filter) = create_signal(DateRange::Last30Days);
    
    let apply_filters = move || {
        let filters = Filters {
            agent_id: agent_filter(),
            status: status_filter(),
            date_range: date_filter(),
        };
        on_change(filters);
    };
    
    view! {
        <div class="session-filters">
            <select on:change=move |ev| {
                let value = event_target_value(&ev);
                set_agent_filter(if value == "all" { None } else { Some(value) });
            }>
                <option value="all">All Agents</option>
                /* Populate from backend */
            </select>
            
            <select on:change=move |ev| {
                let value = event_target_value(&ev);
                set_status_filter(if value == "all" { None } else { Some(value) });
            }>
                <option value="all">All Statuses</option>
                <option value="pending">Pending</option>
                <option value="compressed">Compressed</option>
                <option value="archived">Archived</option>
            </select>
            
            <select on:change=move |ev| {
                set_date_filter(DateRange::from_str(&event_target_value(&ev)));
            }>
                <option value="last7">Last 7 days</option>
                <option value="last30" selected>Last 30 days</option>
                <option value="last90">Last 90 days</option>
                <option value="all">All time</option>
            </select>
            
            <button on:click=move |_| apply_filters()>
                "Apply Filters"
            </button>
        </div>
    }
}
```

**Estimated**: ~100 lines

---

### 8. SearchBar

**File**: `interfaces/webchat/src/platform/wide/views/settings/sessions/search.rs`

**Component**:
```rust
#[component]
pub fn SearchBar(
    on_search: impl Fn(String) + 'static,
) -> impl IntoView {
    let (query, set_query) = create_signal(String::new());
    
    let handle_search = move || {
        on_search(query());
    };
    
    view! {
        <div class="search-bar">
            <input
                type="text"
                placeholder="Search transcripts..."
                value={query}
                on:input=move |ev| set_query(event_target_value(&ev))
                on:keydown=move |ev| {
                    if ev.key() == "Enter" {
                        handle_search();
                    }
                }
            />
            <button on:click=move |_| handle_search()>
                "Search"
            </button>
        </div>
    }
}
```

**Estimated**: ~50 lines

---

## Backend RPC Handlers

### File: `src/gateway/handlers/memory/sessions.rs`

```rust
use crate::gateway::JsonRpcRequest;
use crate::memory::store::markdown::MarkdownSessionStore;
use crate::memory::store::session::{RawSessionStore, SessionMetadata};
use serde::{Deserialize, Serialize};

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct ListSessionsParams {
    pub agent_id: Option<String>,
    pub workspace_id: Option<String>,
    pub start_date: Option<i64>,
    pub end_date: Option<i64>,
    pub compression_status: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ListSessionsResult {
    pub sessions: Vec<SessionMetadata>,
    pub total: usize,
    pub has_more: bool,
}

#[derive(Debug, Deserialize)]
pub struct GetSessionParams {
    pub session_id: String,
}

#[derive(Debug, Serialize)]
pub struct SessionData {
    pub metadata: SessionMetadata,
    pub transcript: String,
    pub tool_outputs: String,
    pub attachments: Vec<AttachmentRef>,
}

#[derive(Debug, Deserialize)]
pub struct SearchSessionsParams {
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub session_id: String,
    pub snippet: String,
    pub score: f32,
    pub metadata: SessionMetadata,
}

// ============================================================================
// RPC Handlers
// ============================================================================

/// memory.sessions.list
pub async fn handle_list_sessions(
    store: Arc<MarkdownSessionStore>,
    params: ListSessionsParams,
) -> Result<ListSessionsResult, JsonRpcError> {
    let limit = params.limit.unwrap_or(20).min(100);
    let offset = params.offset.unwrap_or(0);
    
    let sessions = store
        .list_sessions(
            params.agent_id.as_deref(),
            params.workspace_id.as_deref(),
            params.start_date,
            params.end_date,
            params.compression_status.as_deref(),
            limit,
            offset,
        )
        .await
        .map_err(|e| JsonRpcError::internal_error(e.to_string()))?;
    
    let has_more = sessions.len() == limit;
    let total = offset + sessions.len();
    
    Ok(ListSessionsResult {
        sessions,
        total,
        has_more,
    })
}

/// memory.sessions.get
pub async fn handle_get_session(
    store: Arc<MarkdownSessionStore>,
    params: GetSessionParams,
) -> Result<SessionData, JsonRpcError> {
    let metadata = store
        .get_session_metadata(&params.session_id)
        .await
        .map_err(|e| JsonRpcError::internal_error(e.to_string()))?;
    
    let transcript = store
        .read_transcript(&params.session_id)
        .await
        .map_err(|e| JsonRpcError::internal_error(e.to_string()))?;
    
    let tool_outputs = store
        .read_tool_outputs(&params.session_id)
        .await
        .unwrap_or_default();
    
    let attachments = store
        .list_attachments(&params.session_id)
        .await
        .unwrap_or_default();
    
    Ok(SessionData {
        metadata,
        transcript,
        tool_outputs,
        attachments,
    })
}

/// memory.sessions.search
pub async fn handle_search_sessions(
    store: Arc<MarkdownSessionStore>,
    params: SearchSessionsParams,
) -> Result<Vec<SearchResult>, JsonRpcError> {
    let limit = params.limit.unwrap_or(10).min(50);
    
    let results = store
        .search_transcripts(&params.query, limit)
        .await
        .map_err(|e| JsonRpcError::internal_error(e.to_string()))?;
    
    // Enrich with metadata
    let mut enriched = Vec::new();
    for result in results {
        if let Ok(metadata) = store.get_session_metadata(&result.session_id).await {
            enriched.push(SearchResult {
                session_id: result.session_id,
                snippet: result.snippet,
                score: result.score,
                metadata,
            });
        }
    }
    
    Ok(enriched)
}

/// Download attachment binary
pub async fn handle_download_attachment(
    store: Arc<MarkdownSessionStore>,
    attachment_id: &str,
) -> Result<Vec<u8>, JsonRpcError> {
    store
        .get_attachment(attachment_id)
        .await
        .map_err(|e| JsonRpcError::internal_error(e.to_string()))
}
```

**Estimated**: ~300 lines

---

## RPC Registration

### File: `src/gateway/router.rs` (modify existing)

```rust
// Register memory.sessions.* methods
router.register("memory.sessions.list", |req| async move {
    let params: ListSessionsParams = parse_params(req.params)?;
    let store = get_markdown_store()?;
    let result = handle_list_sessions(store, params).await?;
    Ok(serde_json::to_value(result)?)
});

router.register("memory.sessions.get", |req| async move {
    let params: GetSessionParams = parse_params(req.params)?;
    let store = get_markdown_store()?;
    let result = handle_get_session(store, params).await?;
    Ok(serde_json::to_value(result)?)
});

router.register("memory.sessions.search", |req| async move {
    let params: SearchSessionsParams = parse_params(req.params)?;
    let store = get_markdown_store()?;
    let result = handle_search_sessions(store, params).await?;
    Ok(serde_json::to_value(result)?)
});
```

---

## HTTP Route for Attachment Download

### File: `src/gateway/http_server.rs` (modify existing)

```rust
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
};

async fn download_attachment(
    Path(attachment_id): Path<String>,
    State(store): State<Arc<MarkdownSessionStore>>,
) -> impl IntoResponse {
    match store.get_attachment(&attachment_id).await {
        Ok(data) => {
            let mime_type = infer_mime_type(&attachment_id);
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime_type)],
                data,
            ).into_response()
        }
        Err(_) => {
            (StatusCode::NOT_FOUND, "Attachment not found").into_response()
        }
    }
}

// Add to router
app.route("/api/memory/sessions/attachment/:id", get(download_attachment))
```

---

## Styling

### File: `interfaces/webchat/assets/styles/sessions.css`

```css
.sessions-view {
    height: 100vh;
    display: flex;
    flex-direction: column;
}

.sessions-split {
    display: flex;
    flex: 1;
    overflow: hidden;
}

.session-list {
    width: 40%;
    border-right: 1px solid #ddd;
    overflow-y: auto;
}

.session-card {
    padding: 16px;
    border-bottom: 1px solid #eee;
    cursor: pointer;
    transition: background 0.2s;
}

.session-card:hover {
    background: #f9f9f9;
}

.session-card.selected {
    background: #e3f2fd;
    border-left: 3px solid #2196F3;
}

.session-detail {
    flex: 1;
    display: flex;
    flex-direction: column;
}

.tabs {
    display: flex;
    border-bottom: 1px solid #ddd;
}

.tabs button {
    padding: 12px 24px;
    border: none;
    background: none;
    cursor: pointer;
}

.tabs button.active {
    border-bottom: 2px solid #2196F3;
    color: #2196F3;
}

.tab-content {
    flex: 1;
    overflow-y: auto;
    padding: 20px;
}

/* Transcript Viewer */
.transcript-viewer {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
}

.transcript-content h2 {
    color: #666;
    font-size: 0.9em;
    margin-top: 24px;
    margin-bottom: 8px;
}

.transcript-content p {
    line-height: 1.6;
    margin-bottom: 16px;
}

/* Tool Outputs */
.tool-output-card {
    background: #f5f5f5;
    border-radius: 8px;
    padding: 16px;
    margin-bottom: 16px;
}

.tool-output-card .tool-stdout pre {
    background: #fff;
    padding: 12px;
    border-radius: 4px;
    overflow-x: auto;
}

.tool-output-card .tool-stderr.error pre {
    background: #ffebee;
    color: #c62828;
}

/* Attachment Gallery */
.attachment-gallery {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: 16px;
}

.attachment-card {
    border: 1px solid #ddd;
    border-radius: 8px;
    padding: 12px;
    text-align: center;
}

.attachment-preview-image {
    max-width: 100%;
    max-height: 150px;
    border-radius: 4px;
}

.download-btn {
    margin-top: 8px;
    padding: 8px 16px;
    background: #2196F3;
    color: white;
    border: none;
    border-radius: 4px;
    cursor: pointer;
}

.download-btn:hover {
    background: #1976D2;
}
```

---

## Implementation Timeline

### Day 1: Backend RPC Handlers
- Morning: Implement `handle_list_sessions` + `handle_get_session`
- Afternoon: Implement `handle_search_sessions` + attachment download route
- Evening: Register RPC methods in router

### Day 2: Frontend Components (Part 1)
- Morning: `SessionsView` + `SessionList` + `SessionCard`
- Afternoon: `SessionDetail` + tab switching
- Evening: `SessionFilters`

### Day 3: Frontend Components (Part 2)
- Morning: `TranscriptViewer` with pulldown-cmark
- Afternoon: `ToolOutputsTab`
- Evening: `AttachmentGallery`

### Day 4: Integration & Polish
- Morning: Wire up RPC calls
- Afternoon: CSS styling and responsive design
- Evening: Cross-browser testing

### Day 5: Testing & Documentation
- Morning: End-to-end UI testing
- Afternoon: Fix bugs
- Evening: Update user documentation

---

## Subagent Task Breakdown

### Task D1: Backend RPC Handlers
**Owner**: Subagent  
**Duration**: 4 hours  
**Deliverable**: 3 RPC handlers + HTTP route for attachments

### Task D2: Frontend - List & Filters
**Owner**: Subagent  
**Duration**: 4 hours  
**Deliverable**: SessionList, SessionCard, SessionFilters components

### Task D3: Frontend - Detail View
**Owner**: Subagent  
**Duration**: 4 hours  
**Deliverable**: SessionDetail with 3 tabs

### Task D4: Frontend - Transcript Viewer
**Owner**: Subagent  
**Duration**: 3 hours  
**Deliverable**: TranscriptViewer with Markdown rendering

### Task D5: Frontend - Tool Outputs & Attachments
**Owner**: Subagent  
**Duration**: 3 hours  
**Deliverable**: ToolOutputsTab + AttachmentGallery

### Task D6: Integration & Testing
**Owner**: Subagent  
**Duration**: 4 hours  
**Deliverable**: End-to-end working UI

---

**Total Estimated Effort**: 22 hours across 6 tasks  
**Elapsed Time**: 4-5 days (with parallelization)

---

**Prerequisites for Execution**:
1. Phase A + B compilation verified ✅
2. Phase C tests passing ⏳
3. Real macOS test successful ⏳

---

*This plan will be executed after Phase C is complete.*
