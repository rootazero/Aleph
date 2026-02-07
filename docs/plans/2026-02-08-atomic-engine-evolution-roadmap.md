# Atomic Engine Evolution Roadmap

> **文档状态**: 设计规划
> **创建日期**: 2026-02-08
> **作者**: Claude Sonnet 4.5
> **基于**: [Atomic Engine Implementation Status](../ATOMIC_ENGINE_IMPLEMENTATION_STATUS.md)

---

## 执行摘要

Atomic Engine 已成功完成所有 5 个阶段的实施，性能指标全面超越设计目标（L1 路由快 10,000 倍，Token 节省 99.49%，缓存命中率 87.5%）。本文档规划未来 12+ 个月的演进方向，分为三个阶段：

- **短期（3-6 个月）**：智能化 - 工具扩展、ML 规则生成、错误修复策略
- **中期（6-12 个月）**：平台化 - SDK 化、多语言支持、分布式执行
- **长期（12+ 个月）**：自主化 - AI 规则生成、环境快照、预计算优化

---

## 目录

1. [整体架构愿景](#1-整体架构愿景)
2. [短期演进（3-6 个月）](#2-短期演进3-6-个月)
3. [中期演进（6-12 个月）](#3-中期演进6-12-个月)
4. [长期演进（12+ 个月）](#4-长期演进12-个月)
5. [性能指标演进预测](#5-性能指标演进预测)
6. [技术债务和风险](#6-技术债务和风险)
7. [资源需求估算](#7-资源需求估算)
8. [实施建议](#8-实施建议)

---

## 1. 整体架构愿景

### 1.1 三大演进方向

基于当前 Atomic Engine 的成功，我们看到三个主要演进方向：

#### 1.1.1 智能化演进 - 从规则驱动到 AI 驱动

当前的 L1/L2 路由依赖手写规则（keyword matching）。演进方向是让 AI 自动学习和生成规则，实现真正的"自我进化"。

#### 1.1.2 平台化演进 - 从内部引擎到开放 SDK

将 Atomic Engine 从 Aleph 的内部组件演进为独立的 SDK，让其他 AI Agent 项目也能受益。

#### 1.1.3 分布式演进 - 从单机执行到云端协同

支持跨设备、跨会话的缓存共享和分布式执行，实现"大脑在云端，手脚在身边"的完整形态。

### 1.2 依赖关系图

```
短期（3-6月）          中期（6-12月）         长期（12+月）
┌─────────────┐       ┌─────────────┐       ┌─────────────┐
│ 工具扩展     │──────>│ SDK 化      │──────>│ AI 规则生成 │
│ ML 规则生成  │       │ 多语言支持   │       │ 预计算优化   │
│ 错误修复策略 │       │ 分布式执行   │       │ 环境快照     │
└─────────────┘       └─────────────┘       └─────────────┘
      │                      │                      │
      └──────────────────────┴──────────────────────┘
                    共享基础设施
```

### 1.3 整体架构演进

```
Phase 0 (已完成)          Phase 1 (3-6月)         Phase 2 (6-12月)        Phase 3 (12+月)
┌─────────────┐          ┌─────────────┐         ┌─────────────┐        ┌─────────────┐
│ 基础引擎     │          │ 智能化       │         │ 平台化       │        │ 自主化       │
├─────────────┤          ├─────────────┤         ├─────────────┤        ├─────────────┤
│ L1/L2 路由   │──────────>│ 工具扩展     │─────────>│ SDK 化      │────────>│ AI 规则生成 │
│ 4 种原子操作 │          │ ML 规则生成  │         │ 多语言支持   │        │ 环境快照     │
│ 手写规则     │          │ 错误修复策略 │         │ 分布式执行   │        │ 预计算优化   │
│ 99.49% 节省  │          │              │         │              │        │              │
└─────────────┘          └─────────────┘         └─────────────┘        └─────────────┘
```

---

## 2. 短期演进（3-6 个月）

### 2.1 扩展原子工具：Search, Replace, Move

#### 2.1.1 设计思路

当前 Atomic Engine 支持 4 种原子操作（Read/Write/Edit/Bash），我们需要扩展到文件系统操作领域。新增三个原子工具：

1. **AtomicSearch** - 语义化搜索，支持正则、模糊匹配、AST 级别的代码搜索
2. **AtomicReplace** - 批量替换，支持跨文件、带预览、可回滚
3. **AtomicMove** - 文件/目录移动，自动处理导入路径更新（特别是 Rust 的 `mod` 声明）

#### 2.1.2 架构设计

```rust
// core/src/engine/atomic_tools/
pub enum AtomicTool {
    Read(ReadAction),
    Write(WriteAction),
    Edit(EditAction),
    Bash(BashAction),
    Search(SearchAction),    // 新增
    Replace(ReplaceAction),  // 新增
    Move(MoveAction),        // 新增
}

pub struct SearchAction {
    pattern: SearchPattern,  // Regex | Fuzzy | AST
    scope: SearchScope,      // File | Directory | Workspace
    filters: Vec<FileFilter>,
}

pub struct ReplaceAction {
    search: SearchAction,
    replacement: String,
    preview: bool,           // 生成 diff 预览
    dry_run: bool,
}

pub struct MoveAction {
    source: PathBuf,
    destination: PathBuf,
    update_imports: bool,    // 自动更新引用
    create_parent: bool,
}
```

#### 2.1.3 L2 路由规则示例

```rust
// "find all TODO comments" -> AtomicSearch
KeywordRule::new(
    r"(find|search|grep).*TODO",
    AtomicTool::Search(SearchAction {
        pattern: SearchPattern::Regex(r"TODO:.*"),
        scope: SearchScope::Workspace,
        filters: vec![FileFilter::Code],
    })
)
```

#### 2.1.4 预期收益

- 覆盖更多文件操作场景
- 减少 L3 LLM 调用次数
- 提升用户体验（批量操作、预览、回滚）

---

### 2.2 基于机器学习的规则生成

#### 2.2.1 当前痛点

现在的 L2 路由规则是手写的（如 `git status` → BashAction），每次添加新规则需要修改代码。我们需要让系统自动学习用户的命令模式。

#### 2.2.2 设计方案：轻量级 ML 模型

不使用重型深度学习，而是采用**增量学习 + 特征工程**的方式：

```rust
// core/src/engine/ml_router/
pub struct MLRouter {
    feature_extractor: FeatureExtractor,
    classifier: NaiveBayesClassifier,  // 轻量级分类器
    training_buffer: Vec<TrainingExample>,
}

pub struct FeatureExtractor {
    // 从用户输入提取特征
    fn extract(&self, input: &str) -> FeatureVector {
        FeatureVector {
            keywords: self.extract_keywords(input),
            intent: self.detect_intent(input),  // read/write/execute
            entities: self.extract_entities(input),  // 文件路径、命令名
            context: self.get_session_context(),
        }
    }
}

pub struct TrainingExample {
    input: String,
    features: FeatureVector,
    action: AtomicAction,
    success: bool,
    latency: Duration,
}
```

#### 2.2.3 学习流程

1. **被动学习**：每次 L3 LLM 调用后，记录 `(input, features, action, success)`
2. **特征提取**：提取关键词、意图、实体
3. **模型训练**：每 100 个样本触发一次增量训练
4. **规则生成**：置信度 > 0.85 的模式自动转为 L2 规则

#### 2.2.4 示例

```
用户输入: "show me the git log"
特征: keywords=[show, git, log], intent=read, entity=git
L3 执行: BashAction("git log")
成功后: 学习到 "show.*git.*log" -> BashAction("git log")
下次: L2 直接路由，无需 LLM
```

#### 2.2.5 预期收益

- L2 命中率从 87.5% 提升到 95%+
- 规则维护成本降低 50%
- 自动适应用户习惯

---

### 2.3 更复杂的错误修复策略

#### 2.3.1 当前能力

现在的自愈机制比较简单，只能处理 "目录不存在" 这类基础错误（自动 `mkdir`）。我们需要支持更复杂的错误场景。

#### 2.3.2 设计方案：错误模式库 + 修复策略链

```rust
// core/src/engine/self_healing/
pub struct ErrorPattern {
    matcher: ErrorMatcher,
    fix_strategy: FixStrategy,
    confidence: f32,
}

pub enum ErrorMatcher {
    Regex(Regex),
    ExitCode(i32),
    OutputContains(String),
    Composite(Vec<ErrorMatcher>),  // 组合匹配
}

pub enum FixStrategy {
    Simple(SimpleFixAction),       // 单步修复
    Chain(Vec<FixStrategy>),       // 策略链
    Conditional(Box<ConditionalFix>),  // 条件分支
    LLMAssisted(LLMFixRequest),    // LLM 辅助修复
}

pub struct SimpleFixAction {
    action: AtomicAction,
    description: String,
}
```

#### 2.3.3 内置错误模式示例

```rust
// 1. 权限错误
ErrorPattern {
    matcher: ErrorMatcher::OutputContains("Permission denied"),
    fix_strategy: FixStrategy::Chain(vec![
        FixStrategy::Simple(SimpleFixAction {
            action: AtomicAction::Bash("chmod +x {file}"),
            description: "添加执行权限",
        }),
        FixStrategy::Simple(SimpleFixAction {
            action: AtomicAction::Bash("retry original command"),
            description: "重试原命令",
        }),
    ]),
    confidence: 0.9,
}

// 2. 依赖缺失
ErrorPattern {
    matcher: ErrorMatcher::Regex(r"command not found: (\w+)"),
    fix_strategy: FixStrategy::Conditional(Box::new(ConditionalFix {
        condition: "检查包管理器",
        branches: vec![
            ("cargo", FixStrategy::Simple(SimpleFixAction {
                action: AtomicAction::Bash("cargo install {package}"),
                description: "安装 Rust 包",
            })),
            ("npm", FixStrategy::Simple(SimpleFixAction {
                action: AtomicAction::Bash("npm install -g {package}"),
                description: "安装 npm 包",
            })),
        ],
        fallback: FixStrategy::LLMAssisted(LLMFixRequest {
            prompt: "如何安装 {package}？",
        }),
    })),
    confidence: 0.85,
}

// 3. 端口占用
ErrorPattern {
    matcher: ErrorMatcher::OutputContains("Address already in use"),
    fix_strategy: FixStrategy::Chain(vec![
        FixStrategy::Simple(SimpleFixAction {
            action: AtomicAction::Bash("lsof -ti:{port} | xargs kill -9"),
            description: "杀死占用端口的进程",
        }),
        FixStrategy::Simple(SimpleFixAction {
            action: AtomicAction::Bash("retry original command"),
            description: "重试原命令",
        }),
    ]),
    confidence: 0.95,
}
```

#### 2.3.4 修复流程

```
执行失败 → 匹配错误模式 → 选择修复策略 → 执行修复 → 重试原命令
    ↓                                                    ↓
置信度 < 0.7                                        成功？
    ↓                                                    ↓
降级到 L3 LLM                                      学习到 L1 缓存
```

#### 2.3.5 预期收益

- 自愈成功率从 70% 提升到 90%+
- 减少用户手动干预
- 提升系统鲁棒性

---

### 2.4 短期演进总结

| 功能 | 描述 | 预期收益 |
|------|------|----------|
| **扩展原子工具** | 新增 Search/Replace/Move 三种原子操作 | 覆盖更多场景，减少 L3 调用 |
| **ML 规则生成** | 轻量级 NaiveBayes 分类器，自动学习用户模式 | L2 命中率从 87.5% → 95%+ |
| **错误修复策略** | 错误模式库 + 修复策略链 + LLM 辅助 | 自愈成功率从 70% → 90%+ |

**关键里程碑**：
- M1: 完成 Search/Replace/Move 工具实现（2 周）
- M2: ML 路由器上线，收集 1000+ 训练样本（4 周）
- M3: 错误模式库达到 20+ 内置模式（6 周）

---

## 3. 中期演进（6-12 个月）

### 3.1 Pi 引擎 SDK 化

#### 3.1.1 设计目标

将 Atomic Engine 从 Aleph 的内部组件演进为独立的 Rust crate，让其他 AI Agent 项目也能集成。

#### 3.1.2 架构设计

```rust
// 新的 crate 结构
aleph-atomic-engine/
├── Cargo.toml
├── src/
│   ├── lib.rs              // 公开 API
│   ├── core/               // 核心引擎（从 Aleph 提取）
│   │   ├── atomic_action.rs
│   │   ├── patch.rs
│   │   ├── reflex_layer.rs
│   │   └── atomic_engine.rs
│   ├── adapters/           // 适配器层
│   │   ├── langchain.rs    // LangChain 集成
│   │   ├── autogen.rs      // AutoGen 集成
│   │   └── custom.rs       // 自定义适配器
│   └── traits/             // 公开 trait
│       ├── executor.rs     // ExecutorTrait
│       └── router.rs       // RouterTrait
├── examples/               // 使用示例
└── docs/                   // API 文档
```

#### 3.1.3 公开 API 设计

```rust
// lib.rs
pub struct AtomicEngineBuilder {
    config: EngineConfig,
    custom_rules: Vec<KeywordRule>,
    ml_enabled: bool,
}

impl AtomicEngineBuilder {
    pub fn new() -> Self { ... }
    
    pub fn with_config(mut self, config: EngineConfig) -> Self { ... }
    
    pub fn add_rule(mut self, rule: KeywordRule) -> Self { ... }
    
    pub fn enable_ml(mut self) -> Self { ... }
    
    pub fn build(self) -> Result<AtomicEngine, BuildError> { ... }
}

// 使用示例
let engine = AtomicEngineBuilder::new()
    .with_config(EngineConfig::default())
    .add_rule(KeywordRule::new("git.*", BashAction))
    .enable_ml()
    .build()?;

let result = engine.execute("git status").await?;
```

#### 3.1.4 集成示例：LangChain

```python
# Python 绑定（通过 PyO3）
from aleph_atomic_engine import AtomicEngine

engine = AtomicEngine.builder() \
    .with_config({"cache_size": 1000}) \
    .enable_ml() \
    .build()

# 集成到 LangChain
from langchain.agents import Tool

atomic_tool = Tool(
    name="atomic_executor",
    func=engine.execute,
    description="Fast atomic action executor with L1/L2 routing"
)
```

#### 3.1.5 发布计划

1. **Phase 1**: 提取核心代码到独立 crate
2. **Phase 2**: 设计公开 API 和 trait 系统
3. **Phase 3**: 编写文档和示例
4. **Phase 4**: 发布到 crates.io
5. **Phase 5**: 提供 Python/JavaScript 绑定

---

### 3.2 多语言支持（Python, JavaScript）

#### 3.2.1 设计目标

让非 Rust 项目也能使用 Atomic Engine，通过语言绑定提供原生体验。

#### 3.2.2 技术方案

```
Rust Core (aleph-atomic-engine)
    ↓
┌───────────────┬───────────────┬───────────────┐
│ Python 绑定    │ JavaScript 绑定│ Go 绑定       │
│ (PyO3)        │ (NAPI-RS)     │ (cgo)         │
└───────────────┴───────────────┴───────────────┘
```

#### 3.2.3 Python 绑定设计（PyO3）

```rust
// bindings/python/src/lib.rs
use pyo3::prelude::*;

#[pyclass]
pub struct PyAtomicEngine {
    inner: AtomicEngine,
}

#[pymethods]
impl PyAtomicEngine {
    #[new]
    fn new(config: Option<PyDict>) -> PyResult<Self> {
        let engine = AtomicEngineBuilder::new()
            .with_config(parse_config(config)?)
            .build()
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;
        Ok(Self { inner: engine })
    }
    
    fn execute(&self, py: Python, input: String) -> PyResult<PyObject> {
        py.allow_threads(|| {
            let result = self.inner.execute(&input)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;
            Ok(result.into_py(py))
        })
    }
    
    fn add_rule(&mut self, pattern: String, action: String) -> PyResult<()> {
        // ...
    }
}

#[pymodule]
fn aleph_atomic_engine(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<PyAtomicEngine>()?;
    Ok(())
}
```

#### 3.2.4 Python 使用示例

```python
# pip install aleph-atomic-engine
from aleph_atomic_engine import AtomicEngine

# 初始化引擎
engine = AtomicEngine(config={
    "cache_size": 1000,
    "ml_enabled": True,
})

# 添加自定义规则
engine.add_rule(
    pattern=r"git.*status",
    action="bash:git status"
)

# 执行命令
result = engine.execute("show me git status")
print(result.output)
print(f"Routed via: {result.layer}")  # L1/L2/L3
```

#### 3.2.5 JavaScript 绑定设计（NAPI-RS）

```rust
// bindings/javascript/src/lib.rs
use napi::bindgen_prelude::*;
use napi_derive::napi;

#[napi]
pub struct JsAtomicEngine {
    inner: AtomicEngine,
}

#[napi]
impl JsAtomicEngine {
    #[napi(constructor)]
    pub fn new(config: Option<Object>) -> Result<Self> {
        let engine = AtomicEngineBuilder::new()
            .with_config(parse_config(config)?)
            .build()
            .map_err(|e| Error::from_reason(e.to_string()))?;
        Ok(Self { inner: engine })
    }
    
    #[napi]
    pub async fn execute(&self, input: String) -> Result<JsExecutionResult> {
        let result = self.inner.execute(&input).await
            .map_err(|e| Error::from_reason(e.to_string()))?;
        Ok(JsExecutionResult::from(result))
    }
}
```

#### 3.2.6 JavaScript 使用示例

```javascript
// npm install @aleph/atomic-engine
const { AtomicEngine } = require('@aleph/atomic-engine');

// 初始化引擎
const engine = new AtomicEngine({
  cacheSize: 1000,
  mlEnabled: true,
});

// 执行命令
const result = await engine.execute('show me git status');
console.log(result.output);
console.log(`Routed via: ${result.layer}`);  // L1/L2/L3
```

#### 3.2.7 发布计划

1. **Python**: 发布到 PyPI，支持 Python 3.8+
2. **JavaScript**: 发布到 npm，支持 Node.js 16+
3. **Go**: 发布到 pkg.go.dev（可选，按需求）

---

### 3.3 分布式执行

#### 3.3.1 设计目标

实现"大脑在云端，手脚在身边"的完整形态，支持跨设备、跨会话的缓存共享和分布式执行。

#### 3.3.2 架构设计

```
┌─────────────────────────────────────────────────────────┐
│                    Cloud Control Plane                   │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  │
│  │ Shared L1    │  │ ML Model     │  │ Rule Sync    │  │
│  │ Cache (Redis)│  │ Training     │  │ Service      │  │
│  └──────────────┘  └──────────────┘  └──────────────┘  │
└─────────────────────────────────────────────────────────┘
                            ↕ WebSocket/gRPC
┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│ Client A     │  │ Client B     │  │ Client C     │
│ (macOS)      │  │ (Linux)      │  │ (Windows)    │
│              │  │              │  │              │
│ Local L1/L2  │  │ Local L1/L2  │  │ Local L1/L2  │
│ Executor     │  │ Executor     │  │ Executor     │
└──────────────┘  └──────────────┘  └──────────────┘
```

#### 3.3.3 核心组件

```rust
// core/src/engine/distributed/

pub struct DistributedEngine {
    local_engine: AtomicEngine,
    cloud_client: CloudClient,
    sync_strategy: SyncStrategy,
}

pub struct CloudClient {
    connection: WebSocketConnection,
    cache_sync: CacheSyncManager,
    rule_sync: RuleSyncManager,
}

pub enum SyncStrategy {
    Eager,      // 立即同步到云端
    Lazy,       // 批量同步（每 5 分钟）
    OnDemand,   // 仅在缓存未命中时查询云端
}

pub struct CacheSyncManager {
    local_cache: DashMap<String, CachedAction>,
    cloud_cache: Arc<RwLock<CloudCacheClient>>,
    sync_interval: Duration,
}

impl CacheSyncManager {
    // 本地缓存未命中时，查询云端
    pub async fn get_or_fetch(&self, key: &str) -> Option<CachedAction> {
        // 1. 查询本地 L1
        if let Some(action) = self.local_cache.get(key) {
            return Some(action.clone());
        }
        
        // 2. 查询云端共享缓存
        if let Ok(action) = self.cloud_cache.read().await.get(key).await {
            // 3. 写入本地缓存
            self.local_cache.insert(key.to_string(), action.clone());
            return Some(action);
        }
        
        None
    }
    
    // 本地学习到新规则后，同步到云端
    pub async fn learn_and_sync(&self, key: String, action: CachedAction) {
        // 1. 写入本地缓存
        self.local_cache.insert(key.clone(), action.clone());
        
        // 2. 根据策略同步到云端
        match self.sync_strategy {
            SyncStrategy::Eager => {
                self.cloud_cache.write().await.put(key, action).await.ok();
            }
            SyncStrategy::Lazy => {
                self.pending_sync.push((key, action));
            }
            SyncStrategy::OnDemand => {
                // 不主动同步
            }
        }
    }
}
```

#### 3.3.4 云端服务设计

```rust
// cloud-service/src/main.rs
use axum::{Router, routing::post};
use redis::AsyncCommands;

#[tokio::main]
async fn main() {
    let redis_client = redis::Client::open("redis://127.0.0.1/").unwrap();
    
    let app = Router::new()
        .route("/cache/get", post(get_cache))
        .route("/cache/put", post(put_cache))
        .route("/rules/sync", post(sync_rules))
        .layer(Extension(redis_client));
    
    axum::Server::bind(&"0.0.0.0:8080".parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
}

async fn get_cache(
    Extension(redis): Extension<redis::Client>,
    Json(req): Json<GetCacheRequest>,
) -> Json<GetCacheResponse> {
    let mut conn = redis.get_async_connection().await.unwrap();
    let value: Option<String> = conn.get(&req.key).await.unwrap();
    
    Json(GetCacheResponse {
        action: value.map(|v| serde_json::from_str(&v).unwrap()),
    })
}
```

#### 3.3.5 隐私和安全

```rust
pub struct PrivacyConfig {
    // 哪些命令可以同步到云端
    allowed_patterns: Vec<Regex>,
    // 哪些命令必须本地执行
    blocked_patterns: Vec<Regex>,
    // 是否加密缓存内容
    encrypt_cache: bool,
}

// 示例配置
PrivacyConfig {
    allowed_patterns: vec![
        Regex::new(r"^git ").unwrap(),
        Regex::new(r"^ls ").unwrap(),
    ],
    blocked_patterns: vec![
        Regex::new(r"password").unwrap(),
        Regex::new(r"secret").unwrap(),
        Regex::new(r"token").unwrap(),
    ],
    encrypt_cache: true,
}
```

#### 3.3.6 性能优化

- **本地优先**：L1/L2 始终在本地执行，只有未命中时才查询云端
- **批量同步**：每 5 分钟批量上传本地学习到的规则
- **增量更新**：只同步变化的规则，不全量同步
- **压缩传输**：使用 gzip 压缩 WebSocket 消息

---

### 3.4 中期演进总结

| 功能 | 描述 | 预期收益 |
|------|------|----------|
| **SDK 化** | 独立 crate，公开 API，适配器层 | 其他 AI Agent 项目可集成 |
| **多语言支持** | Python (PyO3) + JavaScript (NAPI-RS) 绑定 | 扩大用户群，降低使用门槛 |
| **分布式执行** | 云端共享缓存 + 跨设备同步 | 缓存命中率提升 20-30% |

**关键里程碑**：
- M4: 发布 aleph-atomic-engine v0.1.0 到 crates.io（3 个月）
- M5: Python/JS 绑定发布到 PyPI/npm（4 个月）
- M6: 云端服务上线，支持 1000+ 并发用户（6 个月）

---

## 4. 长期演进（12+ 个月）

### 4.1 AI 自动生成反射规则

#### 4.1.1 设计目标

从"人工编写规则 + ML 学习模式"演进到"AI 自动生成和优化规则"，实现真正的自我进化。

#### 4.1.2 架构设计

```rust
// core/src/engine/ai_rule_generator/

pub struct AIRuleGenerator {
    llm_client: LLMClient,
    rule_optimizer: RuleOptimizer,
    performance_tracker: PerformanceTracker,
}

pub struct RuleGenerationRequest {
    // 输入数据
    user_patterns: Vec<UserPattern>,      // 用户历史命令
    execution_traces: Vec<ExecutionTrace>, // 执行轨迹
    performance_metrics: PerformanceMetrics, // 性能指标
    
    // 生成约束
    constraints: GenerationConstraints,
}

pub struct GenerationConstraints {
    max_rules: usize,              // 最多生成多少条规则
    min_confidence: f32,           // 最低置信度阈值
    target_hit_rate: f32,          // 目标缓存命中率
    latency_budget: Duration,      // 延迟预算
}

pub struct GeneratedRule {
    pattern: Regex,
    action: AtomicAction,
    confidence: f32,
    estimated_hit_rate: f32,       // 预估命中率
    reasoning: String,             // AI 生成的推理过程
}
```

#### 4.1.3 AI 生成流程

```
1. 数据收集
   ↓
   收集 1000+ 条用户命令历史
   分析执行轨迹和性能指标
   
2. 模式识别（LLM）
   ↓
   Prompt: "分析这些命令，找出可以优化的模式"
   LLM 输出: [
     "用户经常执行 'cargo test --lib'，可以创建 L2 规则",
     "用户总是在 git commit 前执行 git status，可以合并",
   ]
   
3. 规则生成（LLM）
   ↓
   Prompt: "为这些模式生成 L2 路由规则"
   LLM 输出: KeywordRule {
     pattern: r"cargo\s+test\s+--lib",
     action: BashAction("cargo test --lib"),
     confidence: 0.92,
   }
   
4. 规则验证
   ↓
   在沙箱环境中测试生成的规则
   评估准确率、延迟、命中率
   
5. 规则部署
   ↓
   置信度 > 0.85 → 自动部署到 L2
   置信度 0.70-0.85 → 人工审核后部署
   置信度 < 0.70 → 丢弃
```

#### 4.1.4 Prompt 设计示例

```rust
const RULE_GENERATION_PROMPT: &str = r#"
你是一个专门优化 AI Agent 性能的专家。请分析以下用户命令历史，生成高效的路由规则。

## 用户命令历史（最近 1000 条）
{command_history}

## 当前性能指标
- L1 命中率: {l1_hit_rate}%
- L2 命中率: {l2_hit_rate}%
- 平均延迟: {avg_latency}ms
- Token 消耗: {token_usage}

## 任务
1. 识别高频命令模式（出现 > 10 次）
2. 为每个模式生成一条 L2 路由规则
3. 估算每条规则的命中率和性能提升

## 输出格式（JSON）
{
  "rules": [
    {
      "pattern": "regex pattern",
      "action": "bash:command" | "read:path" | "edit:...",
      "confidence": 0.0-1.0,
      "estimated_hit_rate": 0.0-1.0,
      "reasoning": "为什么生成这条规则"
    }
  ]
}
"#;
```

#### 4.1.5 规则优化器

```rust
pub struct RuleOptimizer {
    // 定期分析规则性能，淘汰低效规则
    fn optimize(&self, rules: &mut Vec<KeywordRule>) {
        // 1. 统计每条规则的命中率
        let stats = self.collect_statistics(rules);
        
        // 2. 淘汰命中率 < 1% 的规则
        rules.retain(|rule| {
            stats.get(&rule.id).map_or(false, |s| s.hit_rate > 0.01)
        });
        
        // 3. 合并相似规则
        self.merge_similar_rules(rules);
        
        // 4. 重新排序（高命中率规则优先）
        rules.sort_by(|a, b| {
            stats.get(&b.id).unwrap().hit_rate
                .partial_cmp(&stats.get(&a.id).unwrap().hit_rate)
                .unwrap()
        });
    }
}
```

#### 4.1.6 安全保障

```rust
pub struct RuleSandbox {
    // 在隔离环境中测试规则
    fn test_rule(&self, rule: &GeneratedRule) -> TestResult {
        // 1. 创建临时文件系统
        let temp_fs = TempFileSystem::new();
        
        // 2. 执行规则
        let result = self.execute_in_sandbox(rule, &temp_fs);
        
        // 3. 验证结果
        TestResult {
            success: result.is_ok(),
            side_effects: temp_fs.list_changes(),
            performance: result.latency,
        }
    }
}
```

---

### 4.2 环境快照与回滚

#### 4.2.1 设计目标

在执行高风险操作前自动创建环境快照，支持一键回滚到任意历史状态，类似 Git 但针对整个文件系统和环境。

#### 4.2.2 架构设计

```rust
// core/src/engine/snapshot/

pub struct SnapshotManager {
    storage: SnapshotStorage,
    diff_engine: DiffEngine,
    restore_engine: RestoreEngine,
}

pub struct Snapshot {
    id: SnapshotId,
    timestamp: DateTime<Utc>,
    trigger: SnapshotTrigger,
    scope: SnapshotScope,
    metadata: SnapshotMetadata,
    diff: FileSystemDiff,
}

pub enum SnapshotTrigger {
    Manual,                    // 用户手动创建
    AutoBeforeRisky,          // 高风险操作前自动创建
    Periodic(Duration),       // 定期快照
    BeforeToolExecution(String), // 特定工具执行前
}

pub enum SnapshotScope {
    FullSystem,               // 完整系统快照（慎用）
    Workspace(PathBuf),       // 工作区快照
    Files(Vec<PathBuf>),      // 特定文件快照
    Environment,              // 环境变量快照
}

pub struct FileSystemDiff {
    added: Vec<PathBuf>,
    modified: Vec<FileDiff>,
    deleted: Vec<PathBuf>,
    permissions: Vec<PermissionChange>,
}

pub struct FileDiff {
    path: PathBuf,
    old_content: Vec<u8>,
    new_content: Vec<u8>,
    patch: Patch,             // 增量 diff
}
```

#### 4.2.3 快照策略

```rust
pub struct SnapshotPolicy {
    // 哪些操作触发自动快照
    risky_operations: Vec<RiskyOperation>,
    // 快照保留策略
    retention: RetentionPolicy,
    // 存储限制
    storage_limit: StorageLimit,
}

pub enum RiskyOperation {
    BashCommand(Regex),       // 匹配危险命令（rm -rf, git reset --hard）
    FileDelete,               // 文件删除
    MassEdit(usize),          // 批量编辑（> N 个文件）
    SystemCommand,            // 系统级命令
}

pub struct RetentionPolicy {
    keep_last_n: usize,       // 保留最近 N 个快照
    keep_daily: usize,        // 保留最近 N 天的每日快照
    keep_weekly: usize,       // 保留最近 N 周的每周快照
    auto_cleanup: bool,       // 自动清理旧快照
}

// 示例配置
SnapshotPolicy {
    risky_operations: vec![
        RiskyOperation::BashCommand(Regex::new(r"rm\s+-rf").unwrap()),
        RiskyOperation::BashCommand(Regex::new(r"git\s+reset\s+--hard").unwrap()),
        RiskyOperation::FileDelete,
        RiskyOperation::MassEdit(10),
    ],
    retention: RetentionPolicy {
        keep_last_n: 50,
        keep_daily: 7,
        keep_weekly: 4,
        auto_cleanup: true,
    },
    storage_limit: StorageLimit::MaxSize(10 * 1024 * 1024 * 1024), // 10GB
}
```

#### 4.2.4 快照创建流程

```rust
impl SnapshotManager {
    pub async fn create_snapshot(
        &self,
        scope: SnapshotScope,
        trigger: SnapshotTrigger,
    ) -> Result<Snapshot> {
        // 1. 扫描文件系统
        let current_state = self.scan_filesystem(&scope).await?;
        
        // 2. 与上一个快照对比，生成增量 diff
        let diff = if let Some(prev) = self.get_latest_snapshot(&scope).await? {
            self.diff_engine.compute_diff(&prev.state, &current_state)?
        } else {
            // 首次快照，全量存储
            FileSystemDiff::full(current_state)
        };
        
        // 3. 压缩存储
        let compressed = self.compress_diff(&diff)?;
        
        // 4. 保存快照
        let snapshot = Snapshot {
            id: SnapshotId::new(),
            timestamp: Utc::now(),
            trigger,
            scope,
            metadata: SnapshotMetadata::from_diff(&diff),
            diff: compressed,
        };
        
        self.storage.save(&snapshot).await?;
        
        Ok(snapshot)
    }
}
```

#### 4.2.5 回滚流程

```rust
impl RestoreEngine {
    pub async fn restore_snapshot(
        &self,
        snapshot_id: SnapshotId,
        options: RestoreOptions,
    ) -> Result<RestoreReport> {
        // 1. 加载快照
        let snapshot = self.storage.load(&snapshot_id).await?;
        
        // 2. 创建当前状态的备份快照（以防回滚失败）
        let backup = self.snapshot_manager
            .create_snapshot(snapshot.scope.clone(), SnapshotTrigger::Manual)
            .await?;
        
        // 3. 应用反向 diff
        let mut report = RestoreReport::new();
        
        for file_diff in snapshot.diff.modified.iter().rev() {
            match self.restore_file(file_diff, &options).await {
                Ok(_) => report.success.push(file_diff.path.clone()),
                Err(e) => {
                    report.failures.push((file_diff.path.clone(), e));
                    if !options.continue_on_error {
                        // 回滚失败，恢复到备份
                        self.restore_snapshot(backup.id, RestoreOptions::default()).await?;
                        return Err(RestoreError::PartialRestore(report));
                    }
                }
            }
        }
        
        Ok(report)
    }
}
```

#### 4.2.6 用户界面

```rust
// CLI 命令
// aleph snapshot list
// aleph snapshot create --scope workspace
// aleph snapshot restore <snapshot-id>
// aleph snapshot diff <snapshot-id-1> <snapshot-id-2>

// 自动快照示例
用户: "删除所有 .tmp 文件"
系统: [自动创建快照 snap_20260207_1234]
系统: 执行 "find . -name '*.tmp' -delete"
系统: 删除了 42 个文件
系统: 快照已保存，可通过 'aleph snapshot restore snap_20260207_1234' 恢复
```

#### 4.2.7 存储优化

```rust
pub struct SnapshotStorage {
    // 使用内容寻址存储（类似 Git）
    content_store: ContentAddressableStorage,
    // 增量压缩
    compression: CompressionEngine,
}

// 相同内容只存储一次
impl ContentAddressableStorage {
    fn store(&mut self, content: &[u8]) -> ContentHash {
        let hash = blake3::hash(content);
        if !self.exists(&hash) {
            self.write(&hash, content);
        }
        hash
    }
}
```

---

### 4.3 预计算路由优化

#### 4.3.1 设计目标

通过分析上下文和用户意图，提前预测可能执行的命令，预热缓存和预计算路由，实现"零延迟"响应。

#### 4.3.2 架构设计

```rust
// core/src/engine/predictive/

pub struct PredictiveRouter {
    context_analyzer: ContextAnalyzer,
    intent_predictor: IntentPredictor,
    cache_prewarmer: CachePrewarmer,
    execution_graph: ExecutionGraph,
}

pub struct ContextAnalyzer {
    // 分析当前会话上下文
    fn analyze(&self, session: &Session) -> SessionContext {
        SessionContext {
            current_directory: session.cwd.clone(),
            recent_commands: session.command_history.last_n(10),
            open_files: session.open_files.clone(),
            git_status: self.get_git_status(&session.cwd),
            time_of_day: Utc::now().hour(),
            user_patterns: self.load_user_patterns(&session.user_id),
        }
    }
}

pub struct IntentPredictor {
    // 基于上下文预测下一步可能的命令
    fn predict_next_commands(
        &self,
        context: &SessionContext,
    ) -> Vec<PredictedCommand> {
        let mut predictions = Vec::new();
        
        // 规则 1: Git 工作流预测
        if context.git_status.has_changes {
            predictions.push(PredictedCommand {
                command: "git status".to_string(),
                probability: 0.85,
                reasoning: "有未提交的更改",
            });
            predictions.push(PredictedCommand {
                command: "git diff".to_string(),
                probability: 0.70,
                reasoning: "通常在 git status 后查看 diff",
            });
        }
        
        // 规则 2: 测试工作流预测
        if context.recent_commands.contains(&"cargo build") {
            predictions.push(PredictedCommand {
                command: "cargo test".to_string(),
                probability: 0.80,
                reasoning: "构建后通常运行测试",
            });
        }
        
        // 规则 3: 时间模式预测
        if context.time_of_day == 9 {  // 早上 9 点
            if let Some(pattern) = context.user_patterns.morning_routine {
                predictions.extend(pattern.commands);
            }
        }
        
        predictions
    }
}
```

#### 4.3.3 预热策略

```rust
pub struct CachePrewarmer {
    precompute_threshold: f32,  // 概率阈值（> 0.7 才预计算）
    max_concurrent: usize,      // 最多并发预计算数量
}

impl CachePrewarmer {
    pub async fn prewarm(
        &self,
        predictions: Vec<PredictedCommand>,
    ) -> PrewarmResult {
        let mut tasks = Vec::new();
        
        for pred in predictions {
            if pred.probability > self.precompute_threshold {
                // 在后台预计算路由
                let task = tokio::spawn(async move {
                    let route = self.compute_route(&pred.command).await;
                    (pred.command, route)
                });
                tasks.push(task);
                
                if tasks.len() >= self.max_concurrent {
                    break;
                }
            }
        }
        
        // 等待所有预计算完成
        let results = futures::future::join_all(tasks).await;
        
        // 写入 L1 缓存
        for (command, route) in results {
            self.cache.insert(command, route);
        }
        
        PrewarmResult {
            prewarmed_count: results.len(),
            cache_hit_rate_improvement: self.estimate_improvement(),
        }
    }
}
```

#### 4.3.4 执行图优化

```rust
pub struct ExecutionGraph {
    // 构建命令依赖图，优化执行顺序
    nodes: HashMap<CommandId, CommandNode>,
    edges: Vec<(CommandId, CommandId)>,
}

pub struct CommandNode {
    command: String,
    dependencies: Vec<CommandId>,
    estimated_duration: Duration,
    can_parallelize: bool,
}

impl ExecutionGraph {
    // 分析命令序列，找出可以并行执行的部分
    pub fn optimize(&self, commands: Vec<String>) -> ExecutionPlan {
        let mut plan = ExecutionPlan::new();
        
        // 1. 构建依赖图
        let graph = self.build_dependency_graph(&commands);
        
        // 2. 拓扑排序
        let sorted = self.topological_sort(&graph);
        
        // 3. 识别可并行的命令
        for level in sorted {
            if level.len() > 1 {
                plan.add_parallel_stage(level);
            } else {
                plan.add_sequential_stage(level);
            }
        }
        
        plan
    }
    
    // 示例：识别 git status 和 git log 可以并行
    fn build_dependency_graph(&self, commands: &[String]) -> Graph {
        let mut graph = Graph::new();
        
        for (i, cmd) in commands.iter().enumerate() {
            let node = CommandNode {
                command: cmd.clone(),
                dependencies: self.find_dependencies(cmd, &commands[..i]),
                estimated_duration: self.estimate_duration(cmd),
                can_parallelize: self.is_parallelizable(cmd),
            };
            graph.add_node(i, node);
        }
        
        graph
    }
}
```

#### 4.3.5 智能预测示例

```rust
// 场景 1: Git 工作流
用户刚执行: "git add ."
系统预测:
  - "git status" (概率 0.90) → 预热
  - "git commit" (概率 0.85) → 预热
  - "git push" (概率 0.60) → 不预热（低于阈值）

// 场景 2: 开发工作流
用户刚执行: "cargo build"
系统预测:
  - "cargo test" (概率 0.80) → 预热
  - "cargo run" (概率 0.70) → 预热
  - "cargo clippy" (概率 0.50) → 不预热

// 场景 3: 文件浏览
用户刚执行: "cd src/"
系统预测:
  - "ls" (概率 0.95) → 预热
  - "git status" (概率 0.70) → 预热
  - "find . -name '*.rs'" (概率 0.60) → 不预热
```

#### 4.3.6 性能监控

```rust
pub struct PredictiveMetrics {
    predictions_made: u64,
    predictions_correct: u64,
    cache_hits_from_prewarm: u64,
    latency_reduction: Duration,
}

impl PredictiveMetrics {
    pub fn accuracy(&self) -> f32 {
        self.predictions_correct as f32 / self.predictions_made as f32
    }
    
    pub fn roi(&self) -> f32 {
        // 投资回报率：预热带来的收益 vs 预热成本
        let benefit = self.cache_hits_from_prewarm as f32 * 100.0; // 每次命中节省 100ms
        let cost = self.predictions_made as f32 * 10.0; // 每次预测消耗 10ms
        benefit / cost
    }
}
```

#### 4.3.7 自适应学习

```rust
pub struct AdaptiveLearning {
    // 根据预测准确率动态调整阈值
    fn adjust_threshold(&mut self, metrics: &PredictiveMetrics) {
        if metrics.accuracy() < 0.5 {
            // 准确率太低，提高阈值（减少预测）
            self.threshold = (self.threshold + 0.1).min(0.95);
        } else if metrics.accuracy() > 0.8 && metrics.roi() > 5.0 {
            // 准确率高且 ROI 好，降低阈值（增加预测）
            self.threshold = (self.threshold - 0.05).max(0.5);
        }
    }
}
```

---

### 4.4 长期演进总结

| 功能 | 描述 | 预期收益 |
|------|------|----------|
| **AI 规则生成** | LLM 自动分析用户模式，生成和优化规则 | 规则维护成本降低 90% |
| **环境快照** | 自动快照 + 一键回滚，类似 Git 但针对文件系统 | 高风险操作零心理负担 |
| **预计算优化** | 上下文感知 + 意图预测 + 缓存预热 | 实现"零延迟"响应体验 |

**关键里程碑**：
- M7: AI 规则生成器上线，自动生成 100+ 规则（9 个月）
- M8: 环境快照系统支持 10GB+ 快照存储（10 个月）
- M9: 预计算路由准确率达到 80%+（12 个月）

---

## 5. 性能指标演进预测

| 指标 | 当前 (Phase 0) | 短期 (Phase 1) | 中期 (Phase 2) | 长期 (Phase 3) |
|------|---------------|---------------|---------------|---------------|
| **L1/L2 命中率** | 87.5% | 95% | 97% | 99% |
| **Token 节省** | 99.49% | 99.6% | 99.7% | 99.8% |
| **平均响应时间** | 1-117 μs | < 1 μs | < 1 μs | 0 μs (预计算) |
| **自愈成功率** | 70% | 90% | 95% | 98% |
| **规则维护成本** | 100% (手写) | 50% (半自动) | 20% (大部分自动) | 10% (AI 驱动) |

---

## 6. 技术债务和风险

| 风险 | 缓解措施 |
|------|----------|
| **ML 模型准确率不足** | 设置置信度阈值 (0.85)，低于阈值降级到 L3 |
| **分布式缓存一致性** | 使用 Redis + 版本号，冲突时本地优先 |
| **AI 生成规则不安全** | 沙箱测试 + 人工审核 + 自动回滚 |
| **快照存储成本过高** | 增量存储 + 内容寻址 + 自动清理策略 |
| **预计算浪费资源** | 动态调整阈值 + ROI 监控 + 自适应学习 |

---

## 7. 资源需求估算

| 阶段 | 开发时间 | 人力 | 基础设施成本 |
|------|----------|------|-------------|
| **短期** | 3-6 个月 | 1-2 人 | 无额外成本 |
| **中期** | 6-12 个月 | 2-3 人 | $100-500/月 (云服务) |
| **长期** | 12+ 个月 | 2-3 人 | $500-2000/月 (LLM API + 存储) |

---

## 8. 实施建议

### 8.1 优先级排序

**P0 (必须实施)**：
- 短期：工具扩展 (Search/Replace/Move)
- 短期：ML 规则生成
- 中期：SDK 化

**P1 (强烈建议)**：
- 短期：错误修复策略
- 中期：多语言支持 (Python)
- 长期：AI 规则生成

**P2 (可选)**：
- 中期：分布式执行
- 长期：环境快照
- 长期：预计算优化

### 8.2 实施路径

```
Month 1-2:  工具扩展 (Search/Replace/Move)
Month 3-4:  ML 规则生成器
Month 5-6:  错误修复策略库
Month 7-9:  SDK 化 + 发布到 crates.io
Month 10-12: Python 绑定 + PyPI 发布
Month 13-15: 分布式执行（可选）
Month 16-18: AI 规则生成器
Month 19-21: 环境快照系统（可选）
Month 22-24: 预计算路由优化（可选）
```

### 8.3 成功标准

**短期（3-6 个月）**：
- ✅ L2 命中率达到 95%+
- ✅ 新增 3 种原子工具
- ✅ 错误模式库包含 20+ 模式

**中期（6-12 个月）**：
- ✅ 发布 aleph-atomic-engine v0.1.0
- ✅ Python 绑定发布到 PyPI
- ✅ 至少 10 个外部项目集成

**长期（12+ 个月）**：
- ✅ AI 自动生成 100+ 规则
- ✅ 预计算准确率 > 80%
- ✅ 规则维护成本降低 90%

---

## 9. 结论

Atomic Engine 的演进路线图涵盖了从智能化、平台化到自主化的完整演进路径。通过三个阶段的逐步实施，我们将实现：

1. **智能化**：从手写规则到 ML 自动学习
2. **平台化**：从内部组件到开放 SDK
3. **自主化**：从被动执行到主动优化

这将使 Atomic Engine 成为 AI Agent 领域的基础设施，为整个生态系统提供高性能、低成本的执行引擎。

---

*文档版本: v1.0*
*最后更新: 2026-02-08*
*下次审查: 2026-05-08 (3 个月后)*
