# Desktop Capabilities Evolution Design

> *"学习 OpenClaw，超越 OpenClaw"*
>
> Date: 2026-02-25
> Status: Approved
> Scope: Cross-platform Browser Runtime + Windows Desktop Bridge + Media Understanding Pipeline + Permission System

---

## 1. Background & Motivation

### 1.1 Current State

Aleph's Desktop Bridge (macOS) is fully implemented with 13 capabilities via UDS JSON-RPC 2.0:
- **Perception**: Screenshot (ScreenCaptureKit), OCR (Vision.framework), AX Tree (NSAccessibility)
- **Action**: Click, TypeText, KeyCombo (CGEvent), LaunchApp, WindowList, FocusWindow (NSWorkspace)
- **Canvas**: Show/Hide/Update (WKWebView overlay with A2UI v0.8)

Aleph's Tauri desktop app has complete bridge infrastructure (UDS server, process supervisor, desktop manager) but only `screenshot` is implemented on Windows — the remaining 12 actions are stubs.

### 1.2 OpenClaw Analysis

OpenClaw (a TypeScript/Node.js personal AI assistant) takes a fundamentally different approach:

| Aspect | OpenClaw | Aleph |
|--------|----------|-------|
| **Desktop Control** | Browser-centric (Playwright/CDP) | System-level (AX + CGEvent) |
| **Browser Automation** | First-class built-in tool, 20+ actions | External MCP (Playwright) |
| **Media Understanding** | Pluggable providers (Claude/Gemini/Ollama) | None |
| **Element Grounding** | ARIA snapshot with ref IDs | AX tree (fixed 5-level depth) |
| **Permission System** | exec-approvals.json + allowlist | Framework exists, not wired |
| **Cross-platform** | Node.js (inherently cross-platform) | Platform-specific bridges |

### 1.3 Key Gaps Identified

1. **No built-in browser automation** — reliance on external MCP Playwright limits control and integration depth
2. **No media understanding pipeline** — Agent can capture screenshots but cannot "understand" them
3. **Incomplete Windows desktop actions** — 12 of 13 actions are stubs in Tauri app
4. **No permission/approval workflow** — safety-critical for desktop automation
5. **No streaming tool execution** — no intermediate result feedback during long operations

### 1.4 Design Philosophy

- **Learn from OpenClaw, don't copy** — adapt patterns to Aleph's Rust + trait-based architecture
- **Browser + Native Desktop dual-channel** — not browser-only like OpenClaw
- **Respect red lines** — R1 (brain-limb separation), R3 (core minimalism), R4 (I/O-only interfaces)
- **CDP is protocol, not platform API** — Browser Runtime in Core doesn't violate R1

---

## 2. Overall Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                      Rust Core (The Brain)                       │
│                                                                 │
│  ┌────────────┐  ┌─────────────┐  ┌────────────────────────┐   │
│  │ Agent Loop  │  │ Dispatcher  │  │     Tool Server        │   │
│  │ (OTAF)     │→ │ (DAG)       │→ │ (AlephTool trait)      │   │
│  └─────┬──────┘  └─────────────┘  └──────────┬─────────────┘   │
│        │                                      │                 │
│  ┌─────┴──────────────────────────────────────┴─────────────┐   │
│  │                 Capability Layer (new)                     │   │
│  │                                                           │   │
│  │  ┌────────────────┐  ┌───────────────┐  ┌─────────────┐  │   │
│  │  │ BrowserRuntime │  │ DesktopBridge │  │   Vision    │  │   │
│  │  │ (CDP client)   │  │ (UDS/IPC)     │  │  Pipeline   │  │   │
│  │  │ [Phase 1]      │  │ [Phase 2]     │  │ [Phase 3]   │  │   │
│  │  └───────┬────────┘  └──────┬────────┘  └──────┬──────┘  │   │
│  │          │                  │                   │         │   │
│  │  ┌───────┴──────────────────┴───────────────────┴──────┐  │   │
│  │  │           ApprovalPolicy [Phase 4]                  │  │   │
│  │  └─────────────────────────────────────────────────────┘  │   │
│  └───────────────────────────────────────────────────────────┘   │
│                                                                 │
└─────────┬──────────────────┬───────────────────┬────────────────┘
          │                  │                   │
  ┌───────▼──────┐  ┌───────▼───────┐  ┌────────▼───────┐
  │  Chromium     │  │  Tauri App    │  │  LLM Provider  │
  │  (local CDP)  │  │  (Windows)    │  │ (Claude/Gemini │
  │              │  │  Swift App    │  │  /Ollama)       │
  │              │  │  (macOS)      │  │                │
  └──────────────┘  └───────────────┘  └────────────────┘
```

### 2.1 Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **Browser Runtime lives in Core** | CDP is a pure TCP/WebSocket protocol — no platform APIs involved. Does not violate R1. |
| **Desktop Bridge stays IPC-separated** | Uses platform-specific system APIs (windows-rs, AppKit). R1 compliance. |
| **Vision Pipeline as provider abstraction** | Pluggable providers, no platform dependency. R3 compliance. |
| **Approval layer wraps all capabilities** | Unified safety for browser + desktop + shell. |
| **UDS for Windows** | Windows 10 1803+ supports AF_UNIX. Keeps Core client unchanged. |

---

## 3. Phase 1: Cross-platform Browser Runtime (CDP)

### 3.1 Why CDP, Not Playwright

| Dimension | CDP | Playwright (via MCP) |
|-----------|-----|---------------------|
| **Control granularity** | Full protocol access | Wrapped, MCP-limited |
| **Cross-platform** | Native TCP/WS | Requires Node.js runtime |
| **Latency** | Direct connection | MCP → Node → Playwright |
| **Dependencies** | Only Chromium (user likely has) | Node.js + npm + Playwright |
| **Aleph philosophy** | R3 core minimalism | Heavy external dependency |

### 3.2 Module Structure

```
core/src/browser/
├── mod.rs              # Module exports + BrowserRuntime struct
├── cdp/
│   ├── mod.rs          # CDP WebSocket client
│   ├── protocol.rs     # CDP protocol types (subset)
│   ├── session.rs      # CDP session management (Target.attachToTarget)
│   └── transport.rs    # WebSocket transport (tokio-tungstenite)
├── runtime.rs          # BrowserRuntime: lifecycle, tab management
├── actions.rs          # High-level actions: click, type, navigate, screenshot
├── snapshot.rs         # ARIA snapshot: accessibility tree → element refs
├── discovery.rs        # Find local Chromium installations
└── error.rs            # BrowserError enum
```

### 3.3 Core API

```rust
pub struct BrowserRuntime {
    transport: CdpTransport,
    tabs: HashMap<String, TabInfo>,
    config: BrowserConfig,
}

impl BrowserRuntime {
    // Lifecycle
    pub async fn start(config: BrowserConfig) -> Result<Self, BrowserError>;
    pub async fn stop(&mut self) -> Result<(), BrowserError>;
    pub fn is_running(&self) -> bool;

    // Tab management
    pub async fn open_tab(&mut self, url: &str) -> Result<TabId, BrowserError>;
    pub async fn close_tab(&mut self, tab: &TabId) -> Result<(), BrowserError>;
    pub async fn list_tabs(&self) -> Vec<TabInfo>;
    pub async fn focus_tab(&mut self, tab: &TabId) -> Result<(), BrowserError>;

    // Navigation
    pub async fn navigate(&mut self, tab: &TabId, url: &str) -> Result<(), BrowserError>;
    pub async fn wait_for_load(&mut self, tab: &TabId, timeout_ms: u64) -> Result<(), BrowserError>;
    pub async fn go_back(&mut self, tab: &TabId) -> Result<(), BrowserError>;
    pub async fn go_forward(&mut self, tab: &TabId) -> Result<(), BrowserError>;

    // Actions (via ARIA ref IDs, not CSS selectors)
    pub async fn click(&mut self, tab: &TabId, target: ActionTarget) -> Result<(), BrowserError>;
    pub async fn type_text(&mut self, tab: &TabId, target: ActionTarget, text: &str) -> Result<(), BrowserError>;
    pub async fn press_key(&mut self, tab: &TabId, key: &str, modifiers: &[Modifier]) -> Result<(), BrowserError>;
    pub async fn scroll(&mut self, tab: &TabId, target: ActionTarget, direction: ScrollDir) -> Result<(), BrowserError>;
    pub async fn hover(&mut self, tab: &TabId, target: ActionTarget) -> Result<(), BrowserError>;
    pub async fn select(&mut self, tab: &TabId, target: ActionTarget, values: &[&str]) -> Result<(), BrowserError>;
    pub async fn fill(&mut self, tab: &TabId, target: ActionTarget, value: &str) -> Result<(), BrowserError>;

    // Perception
    pub async fn screenshot(&mut self, tab: &TabId, opts: ScreenshotOpts) -> Result<ScreenshotResult, BrowserError>;
    pub async fn snapshot(&mut self, tab: &TabId) -> Result<AriaSnapshot, BrowserError>;
    pub async fn evaluate(&mut self, tab: &TabId, js: &str) -> Result<serde_json::Value, BrowserError>;
    pub async fn pdf(&mut self, tab: &TabId, opts: PdfOpts) -> Result<Vec<u8>, BrowserError>;

    // Data
    pub async fn get_cookies(&mut self, tab: &TabId, url: Option<&str>) -> Result<Vec<Cookie>, BrowserError>;
    pub async fn set_cookies(&mut self, cookies: &[Cookie]) -> Result<(), BrowserError>;
    pub async fn get_storage(&mut self, tab: &TabId, kind: StorageKind) -> Result<HashMap<String, String>, BrowserError>;

    // Dialog handling
    pub async fn handle_dialog(&mut self, tab: &TabId, accept: bool, text: Option<&str>) -> Result<(), BrowserError>;
}
```

### 3.4 ARIA Snapshot (Key Innovation from OpenClaw)

The most valuable pattern from OpenClaw's browser tool: converting DOM to structured, actionable element references.

```rust
pub struct AriaSnapshot {
    pub elements: Vec<AriaElement>,
    pub page_title: String,
    pub page_url: String,
    pub focused_ref: Option<String>,
}

pub struct AriaElement {
    pub ref_id: String,              // e.g., "button[0]", "input[email]", "link[3]"
    pub role: String,                // ARIA role: button, textbox, link, heading, etc.
    pub name: String,                // Accessible name (text content)
    pub value: Option<String>,       // Current value (for inputs, selects)
    pub state: ElementState,         // Bitmask: focused, disabled, checked, expanded, etc.
    pub bounds: Option<Rect>,        // Bounding box in viewport coordinates
    pub children: Vec<String>,       // Child ref IDs (for tree structure)
}

pub enum ActionTarget {
    Ref(String),                     // ARIA ref ID (preferred)
    Selector(String),                // CSS selector (fallback)
    Coordinates { x: f64, y: f64 },  // Pixel coordinates (last resort)
}
```

Agent workflow:
1. `snapshot()` → Get structured ARIA tree with ref IDs
2. Agent analyzes elements, identifies target
3. `click(tab, ActionTarget::Ref("button[submit]"))` → Action via ref ID
4. No fragile CSS selectors, no pixel guessing

### 3.5 Chromium Discovery

```rust
pub struct BrowserConfig {
    pub mode: LaunchMode,
    pub user_data_dir: Option<PathBuf>,
    pub cdp_port: u16,                   // default: 9222
    pub headless: bool,                  // default: false
    pub extra_args: Vec<String>,
}

pub enum LaunchMode {
    Auto,                                // Find and launch local Chromium
    Connect { endpoint: String },        // Connect to existing CDP endpoint
    Binary { path: PathBuf },            // Use specific binary
}
```

Discovery order:
1. `ALEPH_CHROME_PATH` environment variable
2. Platform-specific default paths:
   - macOS: `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`
   - Windows: `%ProgramFiles%\Google\Chrome\Application\chrome.exe`
   - Linux: `which google-chrome` or `which chromium`
3. Chromium-based alternatives (Edge, Brave, etc.)
4. Fallback: clear error message suggesting installation

### 3.6 BrowserTool (AlephTool Implementation)

```rust
pub struct BrowserTool {
    runtime: Arc<Mutex<Option<BrowserRuntime>>>,
}

pub struct BrowserArgs {
    pub action: BrowserAction,
    pub tab_id: Option<String>,
    pub url: Option<String>,
    pub ref_id: Option<String>,
    pub text: Option<String>,
    pub key: Option<String>,
    pub modifiers: Option<Vec<String>>,
    pub selector: Option<String>,
    pub timeout_ms: Option<u64>,
    pub headless: Option<bool>,
}

pub enum BrowserAction {
    // Lifecycle
    Start, Stop,
    // Tabs
    OpenTab, CloseTab, ListTabs, FocusTab,
    // Navigation
    Navigate, WaitForLoad, GoBack, GoForward,
    // Actions
    Click, Type, PressKey, Scroll, Hover, Select, Fill,
    // Perception
    Screenshot, Snapshot, Evaluate, Pdf,
    // Data
    GetCookies, SetCookies, GetStorage,
    // Dialog
    HandleDialog,
}
```

### 3.7 Relationship with Existing MCP Playwright

- **Do not remove** MCP Playwright support — backward compatible
- **Add** built-in BrowserTool as the **recommended** approach
- Agent prefers built-in BrowserTool; MCP Playwright as fallback
- Long term: built-in tool naturally supersedes MCP

---

## 4. Phase 2: Windows Desktop Actions

### 4.1 Implementation Matrix

| Capability | macOS (Done) | Windows (To Do) | Windows API |
|------------|-------------|-----------------|-------------|
| Screenshot | ScreenCaptureKit | `xcap` crate (done) | ✅ Already working |
| OCR | Vision.framework | `Windows.Media.Ocr` (WinRT) | windows-rs |
| AX Tree | NSAccessibility | UI Automation API | windows-rs |
| Click | CGEvent | `SendInput` (MOUSEINPUT) | windows-rs |
| TypeText | CGEvent + Unicode | `SendInput` (KEYBDINPUT) | windows-rs |
| KeyCombo | CGEvent + VK | `SendInput` + VK mapping | windows-rs |
| LaunchApp | NSWorkspace | `ShellExecuteW` | windows-rs |
| WindowList | CGWindowListCopy | `EnumWindows` | windows-rs |
| FocusWindow | NSWorkspace + AX | `SetForegroundWindow` | windows-rs |
| Canvas Show | WKWebView NSPanel | Tauri WebView window | tauri API |
| Canvas Hide | NSPanel hide | Tauri window hide | tauri API |
| Canvas Update | WKWebView eval | Tauri WebView eval | tauri API |
| Scroll | (not implemented) | `SendInput` (MOUSEINPUT wheel) | windows-rs |

### 4.2 Module Structure (Tauri Side)

```
apps/desktop/src-tauri/src/bridge/
├── mod.rs                # UDS server + JSON-RPC dispatch (existing)
├── protocol.rs           # JSON-RPC helpers (existing)
├── perception.rs         # Screenshot (existing) + OCR + AX Tree
├── action.rs             # Click, TypeText, KeyCombo, Scroll (new)
├── window_mgmt.rs        # LaunchApp, WindowList, FocusWindow (new)
└── canvas.rs             # Canvas Show/Hide/Update via Tauri WebView (new)
```

### 4.3 Protocol Adaptations for Windows

Current protocol uses macOS-centric concepts. Adaptations needed:

1. **`bundle_id` → `app_identifier`**: Windows uses executable paths or AppUserModelId instead of bundle IDs. The `launch_app` method should accept both.

2. **`window_id` type**: macOS uses `CGWindowID` (u32). Windows `HWND` is a pointer-sized value. Use `String` for cross-platform compatibility.

3. **New `desktop.scroll` method**: Both macOS and Windows lack scroll capability in the current protocol.

### 4.4 IPC: UDS on Windows

Windows 10 1803+ supports AF_UNIX sockets. Benefits:
- Core's `DesktopBridgeClient` requires zero changes
- Same socket path convention (`~/.aleph/bridge.sock` → `%USERPROFILE%\.aleph\bridge.sock`)
- Simpler than Named Pipe for our use case (request/response pattern)

### 4.5 Windows OCR Implementation

```rust
// Using Windows.Media.Ocr via windows-rs
// Windows 10 1809+ ships with built-in OCR engine
// Supports: zh-Hans, zh-Hant, en, ja, ko, and 20+ languages

use windows::Media::Ocr::OcrEngine;
use windows::Globalization::Language;
use windows::Graphics::Imaging::SoftwareBitmap;

async fn ocr_recognize(image_bytes: &[u8], lang: &str) -> Result<OcrResult> {
    let language = Language::CreateLanguage(&HSTRING::from(lang))?;
    let engine = OcrEngine::TryCreateFromLanguage(&language)?;
    let bitmap = decode_png_to_software_bitmap(image_bytes)?;
    let result = engine.RecognizeAsync(&bitmap)?.await?;

    Ok(OcrResult {
        full_text: result.Text()?.to_string(),
        lines: result.Lines()?.into_iter().map(|line| {
            OcrLine {
                text: line.Text().unwrap_or_default().to_string(),
                bounding_box: /* from line.Words() */,
                confidence: /* from word.BoundingRect() */,
            }
        }).collect(),
    })
}
```

### 4.6 Windows UI Automation (AX Tree Equivalent)

```rust
// Windows UI Automation API replaces macOS NSAccessibility
use windows::Win32::UI::Accessibility::*;

fn build_ax_tree(hwnd: Option<HWND>, max_depth: u32) -> Result<AxNode> {
    let automation: IUIAutomation = CoCreateInstance(
        &CUIAutomation8::default(), None, CLSCTX_INPROC_SERVER
    )?;

    let root = match hwnd {
        Some(h) => automation.ElementFromHandle(h)?,
        None => automation.GetFocusedElement()?,
    };

    walk_element(&automation, &root, 0, max_depth)
}

fn walk_element(
    automation: &IUIAutomation,
    element: &IUIAutomationElement,
    depth: u32,
    max_depth: u32,
) -> Result<AxNode> {
    let name = element.CurrentName()?.to_string();
    let control_type = element.CurrentControlType()?;
    let role = uia_control_type_to_role(control_type);
    let bounds = element.CurrentBoundingRectangle()?;

    let children = if depth < max_depth {
        let walker = automation.CreateTreeWalker(&automation.RawViewCondition()?)?;
        collect_children(&walker, element, depth + 1, max_depth)?
    } else {
        vec![]
    };

    Ok(AxNode { role, name, bounds, children, /* ... */ })
}
```

---

## 5. Phase 3: Media Understanding Pipeline

### 5.1 VisionProvider Trait

```rust
#[async_trait]
pub trait VisionProvider: Send + Sync {
    /// Understand/describe an image with a prompt
    async fn understand_image(
        &self,
        image: &ImageInput,
        prompt: &str,
    ) -> Result<VisionResult, VisionError>;

    /// OCR with structured output
    async fn ocr(&self, image: &ImageInput) -> Result<OcrResult, VisionError>;

    /// Provider capabilities
    fn capabilities(&self) -> VisionCapabilities;

    /// Provider name for logging/selection
    fn name(&self) -> &str;
}

pub enum ImageInput {
    Base64 { data: String, format: ImageFormat },
    FilePath(PathBuf),
    Url(String),
}

pub struct VisionResult {
    pub description: String,
    pub elements: Vec<VisualElement>,
    pub confidence: f32,
}

pub struct VisualElement {
    pub label: String,
    pub element_type: String,
    pub bounds: Option<Rect>,
    pub confidence: f32,
}
```

### 5.2 Provider Implementations

| Provider | Strength | Use Case |
|----------|----------|----------|
| **Claude Vision** | Strongest understanding | Complex UI analysis, form understanding |
| **Gemini Vision** | Fast, cost-effective | Batch screenshots, quick checks |
| **Ollama (LLaVA)** | Fully local, no network | Privacy-sensitive, offline |
| **Platform OCR** | Fastest, no API call | Pure text extraction |

### 5.3 Integration with Agent Loop

```
Agent: "Help me fill this form"
  ↓
1. BrowserTool.snapshot() → ARIA elements (structural)
2. BrowserTool.screenshot() → PNG image
3. VisionPipeline.understand(screenshot, "Analyze form fields") → Semantic understanding
4. Agent combines ARIA + Vision → Formulates fill plan
5. BrowserTool.type_text(ref_id, value) → Fill each field
```

**Key Insight**: ARIA Snapshot provides "skeleton" (structure), Vision provides "flesh" (semantics). They complement, not replace, each other.

### 5.4 Module Structure

```
core/src/vision/
├── mod.rs              # VisionPipeline, provider registry
├── provider.rs         # VisionProvider trait
├── providers/
│   ├── claude.rs       # Claude Vision implementation
│   ├── gemini.rs       # Gemini Vision implementation
│   ├── ollama.rs       # Ollama/LLaVA implementation
│   └── platform_ocr.rs # Delegates to Desktop Bridge OCR
├── types.rs            # ImageInput, VisionResult, OcrResult
└── error.rs            # VisionError
```

---

## 6. Phase 4: Permission & Approval Workflow

### 6.1 ApprovalPolicy Trait

```rust
#[async_trait]
pub trait ApprovalPolicy: Send + Sync {
    /// Check if an action is allowed
    async fn check(&self, request: &ActionRequest) -> ApprovalDecision;

    /// Record an action for audit trail
    async fn record(&self, request: &ActionRequest, result: &ActionResult);
}

pub enum ApprovalDecision {
    Allow,
    Deny { reason: String },
    Ask { prompt: String },
}

pub struct ActionRequest {
    pub action_type: ActionType,
    pub target: String,
    pub agent_id: String,
    pub context: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

pub enum ActionType {
    BrowserNavigate,
    BrowserClick,
    BrowserType,
    BrowserEvaluate,
    DesktopClick,
    DesktopType,
    DesktopKeyCombo,
    DesktopLaunchApp,
    ShellExec,
}
```

### 6.2 Configuration

```json
// ~/.aleph/approval-policy.json
{
    "version": 1,
    "defaults": {
        "browser_navigate": "allow",
        "browser_action": "allow",
        "browser_evaluate": "ask",
        "desktop_action": "ask",
        "desktop_launch": "ask",
        "shell_exec": "deny"
    },
    "allowlist": [
        { "type": "browser_navigate", "pattern": "https://*.github.com/*" },
        { "type": "browser_navigate", "pattern": "https://*.google.com/*" },
        { "type": "desktop_launch", "pattern": "com.apple.*" },
        { "type": "desktop_launch", "pattern": "C:\\Program Files\\*" }
    ],
    "blocklist": [
        { "type": "shell_exec", "pattern": "rm -rf *" },
        { "type": "browser_navigate", "pattern": "*://malicious.com/*" }
    ],
    "audit": {
        "enabled": true,
        "log_path": "~/.aleph/audit.log",
        "retain_days": 30
    }
}
```

### 6.3 Integration with Existing Exec System

Aleph already has `core/src/exec/` for shell execution security. Phase 4 will:
- **Reuse** exec infrastructure (allowlist patterns, approval flow)
- **Extend** to browser and desktop actions
- **Unify** under a single `ApprovalPolicy` interface

---

## 7. Competitive Advantage: Learning from OpenClaw, Surpassing OpenClaw

| What OpenClaw Does | What Aleph Does Better |
|--------------------|----------------------|
| Node.js CDP wrapper | **Rust-native CDP**: zero GC pauses, lower latency, memory safety |
| Flattened JSON tool schema | **Type-safe enum dispatch**: compile-time guarantees |
| PeekabooBridge (third-party) | **Self-built AX/UIA Bridge**: no third-party dependency |
| External Tesseract OCR | **Platform-native OCR + LLM Vision dual channel** |
| JSON config permissions | **Trait-based approval system**: programmable, extensible |
| Browser-only desktop control | **Browser + Native Desktop dual channel**: full coverage |
| Node.js extension runtime | **WASM + Node.js**: sandboxed, fast, polyglot |
| Pragmatic architecture | **DDD + POE disciplined layers**: clearer boundaries |

---

## 8. Phased Roadmap

| Phase | Content | Key Deliverables | Dependencies |
|-------|---------|-----------------|--------------|
| **1** | Cross-platform Browser Runtime (CDP) | `core/src/browser/`, `BrowserTool`, ARIA Snapshot | None |
| **2** | Windows Desktop Actions | Tauri stub completion (windows-rs), OCR, UIA, input sim | Phase 1 (optional) |
| **3** | Media Understanding Pipeline | `core/src/vision/`, VisionProvider trait, multi-provider | Phase 1 + Phase 2 |
| **4** | Permission & Approval | `core/src/approval/`, config file, audit trail | Phase 1-3 |

Phase 1 and Phase 2 can be developed in parallel as they have no hard dependencies.

---

## 9. Risk Assessment

| Risk | Mitigation |
|------|-----------|
| CDP protocol complexity | Start with essential subset (Page, DOM, Input, Runtime, Network); expand incrementally |
| Chromium not installed | Clear error messages + auto-discovery of Edge/Brave/Chromium alternatives |
| Windows UDS support | Fallback to Named Pipe if AF_UNIX unavailable (Win10 < 1803) |
| Vision API costs | Local OCR as default; cloud vision opt-in; rate limiting |
| Permission UX | Sensible defaults (browser=allow, desktop=ask, shell=deny); easy override |

---

## 10. References

- [OpenClaw Browser Tool](file:///Volumes/TBU4/Workspace/openclaw) — CDP integration patterns
- [Chrome DevTools Protocol](https://chromedevtools.github.io/devtools-protocol/) — Protocol specification
- [Windows UI Automation](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-uiautomationoverview) — Accessibility API
- [Aleph Desktop Bridge Design](docs/plans/2026-02-24-desktop-bridge-design.md) — Previous design
- [Aleph Architecture](docs/reference/ARCHITECTURE.md) — System architecture
