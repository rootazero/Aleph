[INFERRED] 你的蓝图里有一个我会立即改掉的地方：**不要把 PageSensor 作为浏览器核心抽象。** “Sensor”适合描述只读感知，但 Aleph 还需要 navigate/click/type/evaluate、生命周期、事件和状态迁移。更稳的分层是：  
```

BrowserSession
    │
    ├── PageController   ← 动作
    ├── PageSensor       ← 感知
    └── PageState        ← 当前事实

```
[INFERRED] 这样你提出的“感知探针”思想仍然成立，但不会把以后的人机协同和 Chromium fallback 卡死。  
另外两个调整我建议现在就做：  
[INFERRED] **Stable_ID 不要单纯依赖 Arena 的位置索引。** 页面节点会被删除和复用，最好使用 generation-aware key。generational-arena 当前就是专门解决“删除后槽位复用导致旧 ID 误指新对象”的问题；slotmap 也提供稳定、带 generation 的 key。  
[INFERRED] **UI-Diff 的 canonical representation 不要直接做成 Markdown DSL。** 内部应先是 typed Rust enum；Markdown/XML 只是给 LLM 的 serializer。否则以后 replay、测试、跨进程传输都会被文本格式绑架。  
下面我按照这个原则给你一版第一阶段可以真正作为 aleph-browser-core 起点的设计。  
  
## 一、推荐的 crate 边界  
```

aleph/
└── crates/
    ├── aleph-browser-core/
    │   ├── ids.rs
    │   ├── geometry.rs
    │   ├── node.rs
    │   ├── sensor.rs
    │   ├── diff.rs
    │   ├── identity.rs
    │   ├── action.rs
    │   ├── event.rs
    │   └── lib.rs
    │
    ├── aleph-browser-runtime/
    │
    ├── aleph-browser-obscura/
    │
    ├── aleph-browser-chromium/
    │
    ├── aleph-browser-observation/
    │
    └── aleph-browser-migration/

```
aleph-browser-core 应该做到：  
```

不依赖 Obscura
不依赖 Chromium
不依赖 CEF
不依赖 Wry
不依赖 rstar
不依赖 Tokio

```
[INFERRED] 这样 core 是真正的 browser domain protocol；实现层想怎么换都可以。  
  
## 二、aleph-browser-core/Cargo.toml  
[KNOWN] 当前 rstar 最新稳定版是 0.13.0，提供 n-dimensional R*-tree，并支持 bulk loading、nearest-neighbor / intersection 等空间查询。  
[KNOWN] generational-arena 当前是 0.2.9，核心特性就是 generational index，避免 slot reuse 导致旧引用误指新对象。  
[INFERRED] 我建议 core：  
```

[package]
name = "aleph-browser-core"
version = "0.1.0"
edition = "2024"

[dependencies]
serde = { version = "1.0", features = ["derive"] }
thiserror = "2"
url = { version = "2", features = ["serde"] }
uuid = { version = "1", features = ["v4", "serde"] }

# Stable IDs / in-memory page graph
generational-arena = { version = "0.2", features = ["serde"] }

# Spatial index
rstar = "0.13"

# Efficient byte/image buffers
bytes = "1"

```
[KNOWN] 当前 bytes 为 1.12.1，提供便宜 clone/slice 的引用计数 byte buffer，适合截图、IPC payload 等二进制数据。  
  
## 三、ID 系统  
我建议不要直接：  
```

type NodeId = u64;

```
而是：  
```

// ids.rs

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! define_uuid_id {
    ($name:ident) => {
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            Hash,
            Serialize,
            Deserialize,
        )]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }
    };
}

define_uuid_id!(PageId);
define_uuid_id!(FrameId);
define_uuid_id!(SnapshotId);
define_uuid_id!(ActionId);
define_uuid_id!(SessionId);

```
对于**页面内 SpatialNode**，则不要使用 UUID，而使用 generation-aware arena key：  
```

use generational_arena::Index;

pub type SpatialNodeId = Index;

```
[INFERRED] 这是非常重要的区分：  
```

UUID
    → 跨进程 / 持久化 / session identity

Generational Index
    → 页面内部高速节点 identity

```
  
## 四、几何类型  
```

// geometry.rs

use serde::{Deserialize, Serialize};

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Serialize,
    Deserialize,
)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Serialize,
    Deserialize,
)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Serialize,
    Deserialize,
)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn area(&self) -> f32 {
        self.width.max(0.0) * self.height.max(0.0)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x
            && p.x <= self.right()
            && p.y >= self.y
            && p.y <= self.bottom()
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.right()
            && self.right() > other.x
            && self.y < other.bottom()
            && self.bottom() > other.y
    }
}

```
  
## 五、SpatialNode  
这里我会稍微修改你的设计。  
你原先：  
```

bounds
z-index
role

```
不够。  
[INFERRED] Agent 真正需要的是：  
```

identity
geometry
semantics
visibility
hierarchy
interaction

```
因此：  
```

// node.rs

use serde::{Deserialize, Serialize};

use crate::{
    geometry::Rect,
    ids::SpatialNodeId,
};

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct SpatialNode {
    /// Stable within the current page graph.
    pub id: SpatialNodeId,

    /// Parent in the semantic/layout tree.
    pub parent: Option<SpatialNodeId>,

    /// Children ordered according to document/layout order.
    pub children: Vec<SpatialNodeId>,

    /// Absolute viewport-space bounds.
    pub bounds: Rect,

    /// Stacking order.
    pub z_index: i32,

    /// Semantic role.
    pub role: Option<SemanticRole>,

    /// Accessible / human-readable name.
    pub name: Option<String>,

    /// Text content, if useful.
    pub text: Option<String>,

    /// Whether the node participates in interaction.
    pub interactive: bool,

    /// Whether the element is visible after basic layout pruning.
    pub visible: bool,

    /// Whether the element can currently receive pointer/keyboard input.
    pub hit_testable: bool,

    /// Whether the element is editable.
    pub editable: bool,

    /// Optional native/browser locator.
    pub locator: Option<Locator>,

    /// Layout sequence / document order.
    pub document_order: u32,
}

```
  
## 六、SemanticRole  
```

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
)]
pub enum SemanticRole {
    Document,
    Main,
    Navigation,
    Form,
    Dialog,
    Button,
    Link,
    Textbox,
    Password,
    Checkbox,
    Radio,
    Combobox,
    Listbox,
    Menu,
    MenuItem,
    Tab,
    Heading,
    Image,
    Video,
    Input,
    Select,
    Text,
    Generic,
}

```
  
## 七、Locator  
注意：Locator 只是 **resolution hint**，不是 node identity。  
```

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub enum Locator {
    Css(String),
    XPath(String),
    Text(String),
    Aria {
        role: SemanticRole,
        name: Option<String>,
    },
}

```
因此：  
```

SpatialNodeId

```
代表：  
“当前 snapshot 中的这个节点”  
而：  
```

Locator

```
代表：  
“如果我要重新定位它，可以怎么找”  
这两个概念必须分开。  
  
## 八、Page Graph  
我建议内部不是：  
```

Vec<SpatialNode>

```
而是：  
```

use generational_arena::Arena;

pub struct SpatialGraph {
    pub nodes: Arena<SpatialNode>,
    pub root: Option<SpatialNodeId>,
}

```
[KNOWN] generational-arena 的 key 在插入/删除时带 generation，槽位复用后旧 key 不会错误地访问新对象。  
  
## 九、R-Tree 不直接塞 SpatialNode  
[INFERRED] 这里我会改变你的蓝图。  
不要：  
```

RTree<SpatialNode>

```
而要：  
```

RTree<SpatialIndexEntry>

```
因为 SpatialNode 是 semantic graph，R-tree 是 spatial index。  
```

use rstar::{RTreeObject, AABB};

pub struct SpatialIndexEntry {
    pub node_id: SpatialNodeId,
    pub bounds: AABB<[f32; 2]>,
    pub z_index: i32,
}

```
这样：  
```

SpatialGraph
     │
     ├── hierarchy
     ├── semantics
     └── identity

SpatialIndex
     │
     └── geometry

```
两个数据结构职责非常清晰。  
  
## 十、R-Tree Wrapper  
```

pub struct SpatialIndex {
    tree: rstar::RTree<SpatialIndexEntry>,
}

impl SpatialIndex {
    pub fn query_rect(
        &self,
        rect: Rect,
    ) -> impl Iterator<Item = &SpatialIndexEntry> {
        let envelope = AABB::from_corners(
            [rect.x, rect.y],
            [rect.right(), rect.bottom()],
        );

        self.tree.locate_in_envelope_intersecting(&envelope)
    }
}

```
[KNOWN] rstar 原生支持 envelope intersection、nearest-neighbor 等查询；文档也指出 R-tree 更适合大量查询、较少插入，以及提供 bulk loading 来降低构建常数。  
[INFERRED] 因此你的网页 snapshot 最适合：  
```

构建 Layout Tree
      ↓
一次 bulk_load RTree
      ↓
多个 hit-test / selection / nearest-neighbor

```
而不是每次 DOM mutation 就立即 rebuild 整棵树。  
  
## 十一、PageSensor  
这里我建议重新定义。  
[INFERRED] 不要：  
```

trait PageSensor {
    async fn snapshot();
}

```
太窄。  
应该让它成为**纯感知接口**：  
```

use async_trait::async_trait;

#[async_trait]
pub trait PageSensor: Send + Sync {
    async fn page_url(&self) -> Result<Url, SensorError>;

    async fn title(&self) -> Result<Option<String>, SensorError>;

    async fn snapshot(
        &self,
        options: SnapshotOptions,
    ) -> Result<PageSnapshot, SensorError>;

    async fn accessibility_tree(
        &self,
    ) -> Result<Option<AccessibilityTree>, SensorError>;

    async fn layout_tree(
        &self,
    ) -> Result<Option<LayoutTree>, SensorError>;

    async fn screenshot(
        &self,
        options: ScreenshotOptions,
    ) -> Result<Option<ImageRef>, SensorError>;

    async fn hit_test(
        &self,
        point: Point,
    ) -> Result<Option<SpatialNodeId>, SensorError>;
}

```
  
## 十二、但 PageSensor 不负责 Action  
这一点我要非常坚持：  
```

PageSensor
   ↓
观察

```
而不是：  
```

PageSensor
   ↓
观察 + click + type + navigate

```
[INFERRED] 后者最终会让名字和职责完全不一致。  
所以配套：  
```

#[async_trait]
pub trait PageController: Send + Sync {
    async fn navigate(
        &self,
        url: Url,
        options: NavigateOptions,
    ) -> Result<NavigationResult, BrowserError>;

    async fn click(
        &self,
        target: Target,
    ) -> Result<ActionResult, BrowserError>;

    async fn type_text(
        &self,
        target: Target,
        text: String,
    ) -> Result<ActionResult, BrowserError>;

    async fn press(
        &self,
        target: Option<Target>,
        key: Key,
    ) -> Result<ActionResult, BrowserError>;

    async fn scroll(
        &self,
        direction: ScrollDirection,
        amount: f32,
    ) -> Result<ActionResult, BrowserError>;

    async fn evaluate(
        &self,
        expression: String,
    ) -> Result<serde_json::Value, BrowserError>;
}

```
  
## 十三、这样 Backend 的组合就非常漂亮  
```

pub trait Page:
    PageSensor
    + PageController
{
}

```
然后：  
```

ObscuraPage
   │
   ├── PageSensor
   └── PageController

ChromiumPage
   │
   ├── PageSensor
   └── PageController

```
Agent 只依赖：  
```

Page

```
  
## 十四、PageSnapshot  
```

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct PageSnapshot {
    pub id: SnapshotId,

    pub page_id: PageId,

    pub url: Url,

    pub title: Option<String>,

    pub viewport: Size,

    pub root: Option<SpatialNodeId>,

    pub nodes: Vec<SpatialNode>,

    pub screenshot: Option<ImageRef>,

    pub timestamp_ms: u64,
}

```
这里 nodes: Vec<SpatialNode> 是**导出的 snapshot representation**。  
内部 runtime 可以仍然：  
```

Arena<SpatialNode>

```
这样 serialization 和 runtime data structure 分离。  
  
## 十五、UI-Diff  
你的四态：  
```

[+]
[-]
[*]
[~]

```
我认为很好。  
但内部不要做成文本。  
应该：  
```

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct AlephUIDiff {
    pub page_id: PageId,

    pub from: SnapshotId,

    pub to: SnapshotId,

    pub changes: Vec<UIDiffOp>,
}

```
  
## 十六、UIDiffOp  
```

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub enum UIDiffOp {
    Added {
        node: SpatialNode,
    },

    Removed {
        id: SpatialNodeId,
    },

    Changed {
        id: SpatialNodeId,
        fields: NodeChanges,
    },

    Moved {
        id: SpatialNodeId,
        from: Rect,
        to: Rect,
    },
}

```
这正对应：  
```

[+] Added
[-] Removed
[*] Changed
[~] Moved

```
  
## 十七、NodeChanges  
千万不要：  
```

Changed {
    old: SpatialNode,
    new: SpatialNode,
}

```
这样对 Agent context 非常浪费。  
应该 field-level diff：  
```

#[derive(
    Debug,
    Clone,
    Default,
    Serialize,
    Deserialize,
)]
pub struct NodeChanges {
    pub role: Option<(Option<SemanticRole>, Option<SemanticRole>)>,

    pub name: Option<(Option<String>, Option<String>)>,

    pub text: Option<(Option<String>, Option<String>)>,

    pub visible: Option<(bool, bool)>,

    pub interactive: Option<(bool, bool)>,

    pub bounds: Option<(Rect, Rect)>,

    pub z_index: Option<(i32, i32)>,
}

```
  
## 十八、我会把“Diff DSL”放在另一个层  
例如：  
```

aleph-browser-core
          ↓
AlephUIDiff
          ↓
aleph-browser-observation
          ↓
LLMSerializer
          ↓
[+] button#42 "Submit"
[~] textbox#7 y=420→448
[*] button#9 state=disabled→enabled
[-] dialog#21

```
这才是正确分层。  
  
## 十九、Diff Builder  
```

pub trait DiffBuilder {
    fn diff(
        &self,
        previous: &PageSnapshot,
        current: &PageSnapshot,
    ) -> AlephUIDiff;
}

```
[INFERRED] 这样以后你甚至可以有：  
```

FastDiff
SemanticDiff
VisualDiff
DebugDiff

```
  
## 二十、Perception Tick  
我建议直接把你的“300ms 防抖”变成 core policy：  
```

#[derive(
    Debug,
    Clone,
    Copy,
)]
pub struct PerceptionPolicy {
    pub quiet_window_ms: u64,

    pub max_wait_ms: u64,

    pub min_stable_ticks: u8,
}

```
状态机：  
```

Dirty
  │
  ▼
WaitingForQuiet
  │
  ├── DOM changed → reset timer
  │
  └── quiet
       ↓
     Stable
       ↓
     Snapshot
       ↓
      Diff

```
[INFERRED] 我不会把 300ms 硬编码进 core；它应该只是某个 profile 的默认值。  
  
## 二十一、WebIdentityVault  
这里需要比你原设计更谨慎。  
我不建议：  
```

HashMap<String, String>

```
直接表达所有 storage。  
因为 Cookie、LocalStorage、SessionStorage 的语义不同。  
应该：  
```

#[derive(
    Debug,
    Clone,
    Default,
    Serialize,
    Deserialize,
)]
pub struct WebIdentityVault {
    pub cookies: Vec<StoredCookie>,

    pub local_storage: Vec<StorageEntry>,

    pub session_storage: Vec<StorageEntry>,
}

```
  
## 二十二、Cookie  
```

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct StoredCookie {
    pub name: String,

    pub value: String,

    pub domain: String,

    pub path: String,

    pub secure: bool,

    pub http_only: bool,

    pub same_site: SameSite,

    pub expires_unix_ms: Option<i64>,
}

```
  
## 二十三、StorageEntry  
```

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct StorageEntry {
    pub origin: Url,

    pub key: String,

    pub value: String,
}

```
这里的：  
```

origin

```
非常重要。  
[INFERRED] 不能：  
```

key = "token"
value = "..."

```
然后全局注入。  
应该：  
```

https://example.com
    localStorage
        token=...

https://foo.com
    localStorage
        token=...

```
严格按 origin 隔离。  
  
## 二十四、Vault 不应该存 plaintext secrets 的 debug 表示  
[INFERRED] 再进一步：  
```

pub enum SecretValue {
    InMemory(Zeroizing<String>),
    Reference(SecretRef),
}

```
如果你已有 Aleph secret store，最好让：  
```

WebIdentityVault

```
只保留 reference。  
  
## 二十五、WebIdentityVault API  
```

pub trait IdentityVault: Send + Sync {
    async fn cookies_for(
        &self,
        url: &Url,
    ) -> Result<Vec<StoredCookie>, VaultError>;

    async fn put_cookie(
        &self,
        cookie: StoredCookie,
    ) -> Result<(), VaultError>;

    async fn local_storage_for(
        &self,
        origin: &Url,
    ) -> Result<Vec<StorageEntry>, VaultError>;

    async fn session_storage_for(
        &self,
        origin: &Url,
    ) -> Result<Vec<StorageEntry>, VaultError>;

    async fn export(
        &self,
    ) -> Result<WebIdentityVault, VaultError>;
}

```
  
## 二十六、非常重要：不要试图把 Chromium Session 的所有状态“迁移”  
[INFERRED] 你原来的：  
“前向切换时注入 Vault，然后继续请求”  
这个概念应该改成：  
```

Identity Migration

```
而不是：  
```

Session Migration

```
你能稳定迁移的是：  
```

Cookies
LocalStorage
部分 SessionStorage
Current URL

```
不能保证原样迁移的是：  
```

JS heap
service worker state
WebRTC state
in-memory React state
open sockets
renderer state
canvas state
GPU state

```
所以：  
```

Obscura
   ↓
export identity
   ↓
Chromium
   ↓
reconstruct page state

```
是正确的抽象。  
  
## 二十七、Session migration API  
建议：  
```

pub struct MigrationState {
    pub identity: WebIdentityVault,

    pub pages: Vec<PageMigrationState>,
}


pub struct PageMigrationState {
    pub url: Url,

    pub scroll: Point,
}

```
然后：  
```

#[async_trait]
pub trait SessionMigration {
    async fn export(
        &self,
    ) -> Result<MigrationState, MigrationError>;

    async fn import(
        &self,
        state: MigrationState,
    ) -> Result<(), MigrationError>;
}

```
  
## 二十八、BrowserError  
```

#[derive(
    Debug,
    thiserror::Error,
)]
pub enum BrowserError {
    #[error("navigation failed: {0}")]
    Navigation(String),

    #[error("target not found")]
    TargetNotFound,

    #[error("stale target")]
    StaleTarget,

    #[error("unsupported capability: {0:?}")]
    UnsupportedCapability(BrowserCapability),

    #[error("timeout")]
    Timeout,

    #[error("permission denied")]
    PermissionDenied,

    #[error("policy blocked")]
    PolicyBlocked,

    #[error("engine crashed")]
    EngineCrashed,

    #[error("engine unavailable")]
    EngineUnavailable,

    #[error("evaluation failed: {0}")]
    Evaluation(String),

    #[error("internal error: {0}")]
    Internal(String),
}

```
  
## 二十九、Capability  
```

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
)]
pub enum BrowserCapability {
    JavaScript,
    Dom,
    AccessibilityTree,
    Screenshot,
    Pdf,
    Downloads,
    Uploads,
    WebGL,
    WebGPU,
    WebRTC,
    ServiceWorkers,
    Extensions,
    NetworkInterception,
}

```
  
## 三十、BrowserCapabilities  
```

#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
pub struct BrowserCapabilities {
    pub supported: Vec<BrowserCapability>,
}

```
或者性能更重要时：  
```

pub struct BrowserCapabilities {
    bits: u64,
}

```
[INFERRED] 第一版建议 enum + bitflags，因为 Agent routing 会大量查询 capability。  
  
## 三十一、推荐用 bitflags  
```

bitflags = "2"


bitflags::bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct BrowserCapabilities: u64 {
        const JAVASCRIPT           = 1 << 0;
        const DOM                  = 1 << 1;
        const ACCESSIBILITY        = 1 << 2;
        const SCREENSHOT           = 1 << 3;
        const PDF                  = 1 << 4;
        const DOWNLOADS            = 1 << 5;
        const UPLOADS              = 1 << 6;
        const WEBGL                = 1 << 7;
        const WEBGPU               = 1 << 8;
        const WEBRTC               = 1 << 9;
        const SERVICE_WORKERS      = 1 << 10;
        const EXTENSIONS           = 1 << 11;
        const NETWORK_INTERCEPTION= 1 << 12;
    }
}

```
[INFERRED] 如果需要 serde，再开启 bitflags 的 serde feature。  
  
## 三十二、最后是 Browser Backend  
```

#[async_trait]
pub trait BrowserBackend: Send + Sync {
    fn engine(&self) -> BrowserEngine;

    fn capabilities(&self) -> BrowserCapabilities;

    async fn launch(
        &self,
        config: BrowserConfig,
    ) -> Result<BrowserSessionHandle, BrowserError>;
}

```
  
## 三十三、Session Handle  
这里延续我上一轮强调的 Actor 思路。  
```

#[derive(Clone)]
pub struct BrowserSessionHandle {
    pub id: SessionId,

    // Backend-private command channel.
    // The concrete implementation is hidden.
    pub(crate) inner: Arc<dyn SessionHandleInternal>,
}

```
然后：  
```

pub trait SessionHandleInternal:
    Send + Sync
{
}

```
[INFERRED] 实际项目里甚至可以完全不让这个 type 出现在 public docs 中。  
  
## 三十四、你最终的 Aleph API 会非常简洁  
```

let session = browser
    .create_session(
        BrowserRequirement {
            capabilities: BrowserCapabilities::JAVASCRIPT
                | BrowserCapabilities::DOM,
            ..Default::default()
        }
    )
    .await?;

let page = session.new_page().await?;

page.goto(url, NavigateOptions::default()).await?;

let snapshot = page
    .snapshot(SnapshotOptions::agent_default())
    .await?;

let target = snapshot
    .find_role(SemanticRole::Button, "Submit")?;

page.click(Target::Node(target)).await?;

```
  
## 三十五、Crate 依赖，我建议分成三层  
不要让所有 crate 都吃满依赖。  
```
aleph-browser-core

serde
thiserror
url
uuid
generational-arena
rstar
bitflags
bytes

```
  
```
aleph-browser-runtime

tokio
tracing
futures
dashmap          # 可选
parking_lot      # 可选

```
[KNOWN] Tokio 目前是 1.53.1，定位就是异步 I/O runtime，并提供 scheduler、channels、cancellation/backpressure 等基础设施。  
  
```
aleph-browser-obscura

obscura = {
    git = "https://github.com/h4ckf0r0day/obscura",
    tag = "v0.1.7"
}

```
[KNOWN] Obscura 当前作为 Rust library 使用时就是 git dependency，因为其 V8 构建流程不是普通 crates.io dependency；官方文档还建议 pin tag 以获得可重复构建。  
  
## 三十六、Arena 到底选哪个？  
你的原方案写了“Arena Allocator”。  
我会选择：  
**页面节点：generational-arena**  
[KNOWN] 它直接解决 stable/generational identity，而且删除后不会产生 ABA 式 stale reference 问题。  
```

SpatialGraph
   ↓
generational_arena::Arena<SpatialNode>

```
**不建议 typed-arena**  
[KNOWN] typed-arena 是同类型、dropless arena，主要优势是批量生命周期，而不是你的节点删除/reinsert identity 场景。  
所以你的 use case：  
```

DOM mutation
+
node deletion
+
node recreation
+
stable identity

```
更适合 generational arena。  
  
## 三十七、R-Tree：保留你的选择  
这个我基本赞成。  
[KNOWN] rstar 当前就是成熟的 R*-tree 实现，支持 rectangle intersection、nearest-neighbor、bulk loading 等功能。  
但：  
[INFERRED] 不要让 R-tree 成为你的“真相”。  
应该：  
```

Layout / Semantic Graph
        │
        ├── Arena ← Truth
        │
        └── RTree ← Index

```
类似：  
```

PostgreSQL
   │
   ├── table = truth
   └── index = acceleration

```
  
## 三十八、Wry：我建议把它留在 UI adapter，不要进入 core  
[KNOWN] 当前 Wry 0.56.1 是跨平台 WebView rendering library，底层依赖平台 WebView；其 WebView API 本身提供 evaluate_script、navigation、cookie 等能力。  
因此：  
```

aleph-browser-core
        ✕
        Wry

```
应该：  
```

aleph-browser-ui-wry
        ↓
Human Preview Adapter

```
  
## 三十九、IPC：暂时不要进 core  
你提到 IPC。  
[INFERRED] 我建议：  
```

Browser Core
      ↓
Tokio channels

```
先解决进程内问题。  
只有：  
```

Aleph
   │
   ├── Browser Process
   │
   └── Chromium Process

```
真正需要跨进程时，才引入：  
```

interprocess = "2"

```
[KNOWN] 当前 interprocess 2.4.3 提供跨平台 IPC，并包含异步 I/O、命名 local sockets 等通信能力。  
所以：  
```

core
   ↓
abstract channel

runtime
   ↓
Tokio mpsc

process boundary
   ↓
interprocess

```
是更好的层次。  
  
## 四十、推荐依赖总表  

| 用途 | Crate | 结论 |
| ---------------------------- | ------------------ | ---------------------------------- |
| 序列化 | serde | 必选 |
| Error | thiserror | 必选 |
| URL | url | 必选 |
| Session ID | uuid | 推荐 |
| Stable Node ID | generational-arena | 强烈推荐 |
| Spatial Index | rstar | 强烈推荐 |
| Capability bits | bitflags | 推荐 |
| Binary buffer | bytes | 推荐 |
| Async runtime | tokio | runtime 层 |
| Streams | futures | runtime 层 |
| IPC | interprocess | 仅跨进程需要 |
| Native WebView | wry | UI adapter |
| Fast zero-copy serialization | rkyv | 暂时不要 |
| typed-arena | — | 不建议 |
| slotmap | — | 可选，但 core 已由 generational-arena 覆盖 |
  
[KNOWN] rkyv 确实提供 zero-copy deserialization 和高性能序列化，但我不会在 v0.1 的 core domain API 中加入它；Serde 足以承担持久化/IPC schema，而 rkyv 更适合你以后已经确认 snapshot/IPC 是性能瓶颈的场景。  
  
## 四十一、最终推荐的 lib.rs  
```

pub mod action;
pub mod capability;
pub mod diff;
pub mod error;
pub mod event;
pub mod geometry;
pub mod identity;
pub mod ids;
pub mod navigation;
pub mod node;
pub mod policy;
pub mod sensor;
pub mod snapshot;
pub mod target;

pub use action::*;
pub use capability::*;
pub use diff::*;
pub use error::*;
pub use event::*;
pub use geometry::*;
pub use identity::*;
pub use ids::*;
pub use navigation::*;
pub use node::*;
pub use policy::*;
pub use sensor::*;
pub use snapshot::*;
pub use target::*;

```
  
## 四十二、我建议你现在实际冻结的核心边界  
```

                     Aleph Agent
                          │
                          ▼
                 ┌─────────────────┐
                 │  Browser API    │
                 └────────┬────────┘
                          │
              ┌───────────┼───────────┐
              ▼           ▼           ▼
           Sensor      Controller    Events
              │           │           │
              └───────────┼───────────┘
                          ▼
                     Browser Page
                          │
                ┌─────────┴─────────┐
                ▼                   ▼
          SpatialGraph          RTree Index
                │
         generational Arena
                │
                ▼
          Spatial Snapshot
                │
                ▼
             UI Diff
                │
                ▼
          Agent Context

```
而 Backend：  
```

                    Browser Page
                         │
            ┌────────────┴────────────┐
            ▼                         ▼
        Obscura                    Chromium
            │                         │
      native Rust API             CEF/CDP
            │                         │
            └────────────┬────────────┘
                         ▼
                  same core traits

```
  
## 四十三、我会对你的原方案做的五个关键修改  
[INFERRED]  
**第一，PageSensor 从“浏览器总抽象”降级为只读感知接口。**  
```

PageSensor
PageController
Page

```
三者分离。  
**第二，SpatialNode 的 stable ID 改成 generation-aware，而不是简单 Arena index。**  
这解决 SPA 高频删除/重建造成的 stale target。  
**第三，R-tree 作为 index，而不是 canonical state。**  
```

Arena = truth
RTree = acceleration

```
**第四，AlephUIDiff 内部严格 typed；你的 [+] [-] [*] [~] 只作为 LLM wire format。**  
**第五，WebIdentityVault 变成“identity migration”，而不是“session migration”。**  
```

identity:
cookie
storage
credential reference

session:
JS heap
network sockets
DOM runtime
service worker
GPU

```
这两个概念一定要分开。  
  
## 四十四、最终的最小可行核心  
如果现在只允许我先写 **三个结构体 + 一个 trait**，我会真正冻结成：  
```

// 1. SpatialNode
pub struct SpatialNode {
    pub id: SpatialNodeId,
    pub parent: Option<SpatialNodeId>,
    pub children: Vec<SpatialNodeId>,

    pub bounds: Rect,
    pub z_index: i32,

    pub role: Option<SemanticRole>,
    pub name: Option<String>,
    pub text: Option<String>,

    pub visible: bool,
    pub interactive: bool,
    pub hit_testable: bool,
    pub editable: bool,

    pub locator: Option<Locator>,
    pub document_order: u32,
}


// 2. AlephUIDiff
pub struct AlephUIDiff {
    pub page_id: PageId,
    pub from: SnapshotId,
    pub to: SnapshotId,
    pub changes: Vec<UIDiffOp>,
}


// 3. WebIdentityVault
pub struct WebIdentityVault {
    pub cookies: Vec<StoredCookie>,
    pub local_storage: Vec<StorageEntry>,
    pub session_storage: Vec<StorageEntry>,
}


// 4. PageSensor
#[async_trait]
pub trait PageSensor: Send + Sync {
    async fn page_url(&self)
        -> Result<Url, SensorError>;

    async fn snapshot(
        &self,
        options: SnapshotOptions,
    ) -> Result<PageSnapshot, SensorError>;

    async fn accessibility_tree(
        &self,
    ) -> Result<Option<AccessibilityTree>, SensorError>;

    async fn layout_tree(
        &self,
    ) -> Result<Option<LayoutTree>, SensorError>;

    async fn screenshot(
        &self,
        options: ScreenshotOptions,
    ) -> Result<Option<ImageRef>, SensorError>;

    async fn hit_test(
        &self,
        point: Point,
    ) -> Result<Option<SpatialNodeId>, SensorError>;
}

```
[INFERRED] 这样做以后，**Obscura 可以直接把自己的 native Element/layout 信息翻译成 SpatialNode，Chromium 可以通过 CDP/DOM/A11y/CEF 信息生成同一个 SpatialNode，而 Agent 永远只面对 Aleph 的统一空间语义。** Obscura 官方目前确实提供 native Rust Browser/Page/Element API，并且这条路径可以避免 CDP round-trip，因此这是我更推荐的第一方 backend 接法。  
[rules] 没有需要报告的规则冲突。  
  
我不知道你上一版“设计思路”里是否明确规定了安装包外置这一点；但从你现在补充的要求看，**是可以完全兼容的，而且我建议把它正式写进架构。**  
[INFERRED] 但两者并不是同一个层次的问题：  
```

我的前面设计
= 运行时架构
    Browser Core
      ↓
    Supervisor
      ↓
  Obscura / Chromium

你的补充设计
= 分发/安装架构
    Aleph Installer
      ↓
  下载 Browser Runtime
      ↓
  安装到独立目录

```
二者实际上应该叠加，而不是二选一。  
  
## 我建议 Aleph 最终采用这个模型  
```

                    Aleph Installer
                          │
             ┌────────────┴────────────┐
             │                         │
             ▼                         ▼
       Aleph Core                Browser Runtime
       必须内置                  按需下载/安装
             │                         │
             │              ┌──────────┴──────────┐
             │              │                     │
             │              ▼                     ▼
             │           Obscura              Chromium
             │          Runtime                 Runtime
             │
             └──────────────┬─────────────────────
                            │
                     Browser Supervisor
                            │
               ┌────────────┴────────────┐
               ▼                         ▼
           Obscura                    Chromium
        First-Class                 Escalation

```
[INFERRED] 也就是说：  
**Browser 是 Aleph 的 first-class capability，但 Browser Engine 本身不是 Aleph 主程序 binary 的组成部分。**  
我认为这个模式比把 Obscura 和 Chromium 全部塞进 Aleph 安装包更好。  
  
## 1. 为什么我会赞成你的设计  
你实际上是在做：  
```

Aleph executable
       +
external browser runtimes

```
而不是：  
```

Aleph executable
  ├── Obscura
  └── Chromium

```
这样会有几个很明显的好处。  
**安装包尺寸**  
[INFERRED] Chromium 是最大的负担之一。没有必要让每个 Aleph 用户第一次安装就携带完整 Chromium。  
你的：  
```

Aleph

```
可以非常轻。  
第一次启动：  
```

Browser capability required
        ↓
runtime manager
        ↓
download
        ↓
verify
        ↓
install
        ↓
ready

```
  
## 2. 更重要的是版本解耦  
这是我认为比体积更重要的。  
假设：  
```

Aleph = 0.9
Obscura = 0.3
Chromium = 140

```
如果全部静态打进一个 binary：  
```

Aleph 0.9
   ↓
browser upgrade
   ↓
重新发布 Aleph

```
而外置 runtime：  
```

Aleph 0.9
    │
    ├── Obscura Runtime 0.3.2
    └── Chromium Runtime 140.x

```
可以独立升级。  
[INFERRED] 这也非常适合你现在的双引擎路线，因为 **Obscura 的迭代速度很可能远快于 Aleph 主程序的 release cadence**。  
  
## 3. 我会把它明确叫 Browser Runtime  
不要叫：  
```

browser dependency

```
而是：  
**Aleph Browser Runtime**  
目录可以：  
```

~/.aleph/
├── aleph
├── config/
├── data/
└── runtimes/
    └── browser/
        ├── manifest.json
        ├── obscura/
        │   └── 0.1.x/
        └── chromium/
            └── 140.x/

```
macOS 也可以更正式地放进：  
```

~/Library/Application Support/Aleph/runtimes/

```
Windows：  
```

%LOCALAPPDATA%\Aleph\runtimes\

```
Linux：  
```

~/.local/share/aleph/runtimes/

```
  
## 4. 这里还有一个很重要的修改  
我之前把：  
```

aleph-browser-obscura
aleph-browser-chromium

```
理解成 Cargo crates。  
**这仍然成立。**  
但现在应该进一步区分：  
```

Rust integration layer

```
和：  
```

Browser runtime binary

```
也就是：  
```

Aleph source tree
│
├── aleph-browser-core
├── aleph-browser-runtime
├── aleph-browser-obscura
└── aleph-browser-chromium

```
这些是：  
Rust integration / adapter  
而实际运行时：  
```

runtimes/
├── obscura-runtime
└── chromium-runtime

```
是：  
可下载的外部 runtime artifact  
  
## 5. 于是 architecture 变成四层  
这是我会正式修改上一版设计的地方：  
```

┌─────────────────────────────────────────┐
│             Aleph Agent                 │
├─────────────────────────────────────────┤
│       aleph-browser-core                │
│   Domain / Sensor / Controller / State │
├─────────────────────────────────────────┤
│       aleph-browser-runtime             │
│ Supervisor / Routing / Lifecycle        │
├─────────────────────────────────────────┤
│         Runtime Adapters                │
│ ┌────────────────┐ ┌─────────────────┐ │
│ │ Obscura Adapter│ │ Chromium Adapter│ │
│ └───────┬────────┘ └────────┬────────┘ │
└─────────┼───────────────────┼──────────┘
          │                   │
          ▼                   ▼
┌──────────────────┐  ┌──────────────────┐
│ Obscura Runtime  │  │ Chromium Runtime │
│ external artifact│  │ external artifact│
└──────────────────┘  └──────────────────┘

```
[INFERRED] **这会比“把第三方浏览器库直接编译进 Aleph”干净很多。**  
  
## 6. 但这里有一个关键分歧：Obscura 怎么外置？  
这个需要你特别注意。  
如果你的 Obscura 是：  
```

Rust library

```
那么最简单的方式其实不是：  
```

Aleph
  ↓
启动 obscura executable

```
而是：  
```

Aleph
  ↓
dynamic library / dedicated runtime process

```
具体选哪种，要根据你最终希望：  
* 独立升级  
* 崩溃隔离  
* 内存释放  
* 多 session  
* sandbox  
* cross-platform distribution  
到什么程度来决定。  
  
## 7. 对 Obscura，我更推荐独立进程  
这和之前的：  
```

!Send
LocalSet
V8 isolate

```
设计是可以结合的。  
最终：  
```

Aleph
 │
 │ IPC
 ▼
Obscura Runtime Process
 │
 ├── LocalSet
 ├── V8
 ├── Browser
 └── Pages

```
而不是：  
```

Aleph main process
   └── Obscura runtime

```
[INFERRED] 这样 Obscura 崩溃：  
```

Obscura crash
    ↓
restart runtime
    ↓
restore session

```
不会拖垮 Aleph Agent。  
  
## 8. Chromium 更应该独立进程  
Chromium 天然就是：  
```

Browser Process
Renderer Process
GPU Process
Network Process

```
所以：  
```

Aleph
   │
   └── Chromium Runtime

```
是非常自然的。  
  
## 9. 那么两个 Runtime 都统一成 Process Boundary  
最终：  
```

                         Aleph
                           │
                   Browser Supervisor
                           │
             ┌─────────────┴────────────┐
             │                          │
             ▼                          ▼
      Obscura Process             Chromium Process
             │                          │
        Obscura API                  CEF/CDP
             │                          │
             └─────────────┬────────────┘
                           │
                       Browser API

```
[INFERRED] **我反而很喜欢这个架构。**  
因为这样 BrowserSupervisor 真正成为 runtime supervisor，而不是普通 facade。  
  
## 10. 安装流程应该独立成 Runtime Manager  
新增：  
```

aleph-browser-runtime-manager

```
负责：  
```

discover
download
verify
install
update
rollback
remove
repair

```
接口：  
```

#[async_trait]
pub trait RuntimeManager {
    async fn ensure(
        &self,
        runtime: BrowserRuntime,
    ) -> Result<RuntimeHandle>;

    async fn installed_versions(
        &self,
        runtime: BrowserRuntime,
    ) -> Result<Vec<RuntimeVersion>>;

    async fn update(
        &self,
        runtime: BrowserRuntime,
    ) -> Result<RuntimeHandle>;

    async fn verify(
        &self,
        runtime: BrowserRuntime,
    ) -> Result<RuntimeHealth>;
}

```
  
## 11. Runtime 类型  
```

pub enum BrowserRuntime {
    Obscura,
    Chromium,
}

```
以后：  
```

pub enum BrowserRuntime {
    Obscura,
    Chromium,
    WebKit,
}

```
也不影响 core。  
  
## 12. Runtime Manifest  
这个非常值得做。  
例如：  
```

{
  "runtime": "chromium",
  "version": "140.0.7339.0",
  "platform": "macos-arm64",
  "download": {
    "url": "...",
    "sha256": "..."
  }
}

```
然后安装器：  
```

download
  ↓
SHA256 verify
  ↓
signature verify
  ↓
extract
  ↓
atomic rename

```
  
## 13. 不要直接覆盖旧版本  
应该：  
```

runtimes/chromium/
├── 139.0/
├── 140.0/
└── current -> 140.0/

```
更新：  
```

download 141
   ↓
verify
   ↓
install 141
   ↓
health check
   ↓
current → 141

```
如果失败：  
```

current → 140

```
[INFERRED] 这对一个 Agent 产品尤其重要，因为 Browser runtime 崩了不能把整个 Aleph 更新搞坏。  
  
## 14. 首次安装  
我建议默认策略：  
```

Aleph first launch
        ↓
Does user need Browser?
        │
   ┌────┴────┐
   │         │
  No        Yes
   │         │
 continue  Runtime Manager
             │
       ensure Obscura
             │
          success
             │
         ready

```
而不是安装 Aleph 时强制下载 Chromium。  
  
## 15. Chromium 可以真正做到“按需下载”  
例如：  
```

User asks:
“打开网页让我看看”
        ↓
Chromium required
        ↓
Runtime Manager
        ↓
download Chromium

```
甚至：  
```

Obscura task
   ↓
no Chromium needed

```
一个 Chromium 都不用下载。  
[INFERRED] 这个设计非常符合你最初的“Obscura first-class”理念。  
  
## 16. 更进一步：平台感知安装  
```

runtime/
├── macos-aarch64/
├── macos-x86_64/
├── windows-x86_64/
├── linux-x86_64/
└── linux-aarch64/

```
Manifest：  
```

Aleph Version
Browser Runtime Version
Platform
Architecture
Minimum OS
SHA256
Signature

```
  
## 17. 这样 Cargo dependency 也应该重新分类  
你的 Aleph：  
```

[dependencies]
aleph-browser-core
aleph-browser-runtime

```
而：  
```

Obscura
CEF
Chromium

```
尽量不要成为 Aleph 主程序的巨大 runtime payload。  
[INFERRED] 不过注意：  
**“外部下载”不意味着完全不能有 compile-time Rust dependency。**  
你仍然可能需要：  
```

aleph-browser-obscura

```
作为 adapter，但它最好只承担：  
```

protocol
IPC
runtime launching
serialization
health check

```
而不是把整个第三方 engine 静态链接进 Aleph。  
  
## 18. 这会让 aleph-browser-obscura 变成一个 Runtime Client  
```

aleph-browser-obscura
        │
        ├── spawn obscura-runtime
        ├── IPC
        ├── protocol
        ├── health
        └── lifecycle

```
而：  
```

obscura-runtime

```
自己负责：  
```

Obscura Browser
V8
DOM
Layout
Network
Page

```
  
## 19. 这比之前的设计还更适合你的 “Drop 释放内存”  
你原文里提到：  
Drop 特征强制销毁 Chromium 进程释放内存  
[INFERRED] 如果你采用真正的 process boundary，这个事情会变得非常干净：  
```

Chromium Runtime
      │
      ▼
 BrowserSupervisor
      │
      ▼
 drop BrowserProcessHandle
      │
      ▼
 terminate child process
      │
      ▼
 process memory returned to OS

```
比在 Rust 进程内部：  
```

Arc<...>
drop
hope everything frees

```
可靠得多。  
  
## 20. 所以你的最终架构，我会这样改  
```

                         ALEPH
                           │
                           ▼
                  Browser Supervisor
                           │
                ┌──────────┴───────────┐
                │                      │
                ▼                      ▼
          Runtime Manager        Browser Core
                │                      │
         ┌──────┴──────┐               │
         │             │               │
         ▼             ▼               │
      Obscura       Chromium           │
      Runtime       Runtime             │
         │             │               │
         ▼             ▼               │
   ┌──────────┐   ┌──────────┐         │
   │ Obscura  │   │ Chromium │         │
   │ Process  │   │ Process  │         │
   └──────────┘   └──────────┘         │
         │             │               │
         └──────┬──────┘               │
                │                      │
                └──────────┬───────────┘
                           ▼
                      PageSensor
                      PageController
                           │
                           ▼
                     Agent Context

```
  
## 21. 最后一个重要区别：安装包 vs runtime  
我建议最终把它定义成两个产品组件：  
**Aleph Core**  
```

Aleph executable
Agent
LLM
Tools
Browser Core
Runtime Manager

```
**Aleph Browser Runtime**  
```

Obscura Runtime
Chromium Runtime

```
因此用户看到的是：  
```

Install Aleph
     ↓
Aleph starts
     ↓
Browser Runtime Manager
     ↓
Install required browser engines

```
而不是：  
```

Install Aleph
     ↓
download everything
     ↓
500MB+ package

```
  
## 22. 所以答案是：基本一致，但我现在会把你的想法提升成一个正式架构原则  
[INFERRED]  
你原本的理念：  
**第三方浏览器基座不要内置进 Aleph 安装包，而是在安装/运行时按需拉取。**  
我会正式定义为：  
**External Browser Runtime Architecture**  
并把它作为 Aleph Browser subsystem 的一级设计原则：  
```

                    Aleph
                       │
                       │ stable protocol
                       ▼
              Browser Supervisor
                       │
                Runtime Manager
                       │
          ┌────────────┴────────────┐
          ▼                         ▼
   Obscura Runtime           Chromium Runtime
   external artifact          external artifact
          │                         │
          ▼                         ▼
   First-Class Engine        Capability Escalation

```
[INFERRED] 而且我会进一步建议：**Obscura 和 Chromium 都不要直接作为 Aleph 主进程里的第三方“内嵌库”来处理，而是优先采用独立 runtime process + Aleph-defined IPC protocol。** 对你的双引擎方案而言，这会同时解决版本解耦、崩溃隔离、内存释放、按需安装和引擎升级五个问题。  
你上一轮让我写的启动重构 prompt 也因此需要改一下：应该把 **“External Browser Runtime / Runtime Manager / Runtime Manifest / Process Boundary”** 加到任务的最前面，否则 Codex 很可能按照旧设计把 Obscura crate 直接集成进 Aleph binary。  
