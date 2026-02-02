# POE 第一性原理闭环：契约签署模式设计

> **状态**: Draft
> **日期**: 2026-02-02
> **作者**: Claude + User

---

## 概述

本设计实现 POE 架构的"第一性原理"闭环，通过"契约签署 (Contract Signing)"模式让用户在 AI 执行任务前审批成功标准。

### 核心价值

- **仪式感**: 用户签署"成功契约"，心理上有兜底
- **透明度**: 用户能看到 AI 打算如何验证自己
- **可控性**: 用户可修改约束规则，确保符合预期

### 设计决策摘要

| 决策点 | 选择 |
|--------|------|
| 交互模式 | 审批模式（契约签署） |
| 修改方式 | 混合模式：自然语言优先 + JSON 高级模式 |
| RPC 设计 | 两阶段：prepare → sign/reject |
| 存储策略 | 内存存储，无超时，手动清理 |
| 契约修改 | ManifestBuilder 增量模式 |
| 未来演进 | 预留渐进信任接口 (V1.5 白名单, V2.0 信任分) |

---

## 整体架构

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           CLIENT LAYER                                   │
│   macOS App │ Tauri App │ CLI │ Telegram                                │
│                                                                          │
│   ┌──────────────────────────────────────────────────────────────────┐  │
│   │  📜 契约卡片 UI                                                    │  │
│   │  ┌────────────────────────────────────────────────────────────┐  │  │
│   │  │ 目标: 重构 Auth 模块                                        │  │  │
│   │  │ ─────────────────────────────────────────────────────────  │  │  │
│   │  │ [硬性] src/auth.rs 存在                                     │  │  │
│   │  │ [硬性] 代码不含 "sk-" 字符串                                 │  │  │
│   │  │ [语义] 使用 std::env::var 读取配置                          │  │  │
│   │  │ ─────────────────────────────────────────────────────────  │  │  │
│   │  │ [🖊️ 签署] [✏️ 修改] [❌ 取消]     [📄 查看JSON]             │  │  │
│   │  └────────────────────────────────────────────────────────────┘  │  │
│   └──────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    │ WebSocket (JSON-RPC 2.0)
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                          GATEWAY LAYER                                   │
│                                                                          │
│   poe.prepare ──→ ManifestBuilder ──→ PendingContractStore (内存)       │
│        │                                       │                         │
│        │ ←──────── contract_id + manifest ─────┘                         │
│        │                                                                 │
│   poe.sign ───→ PendingContractStore.take() ──→ PoeManager.execute()    │
│        │                                              │                  │
│   poe.reject ─→ PendingContractStore.remove()        ▼                  │
│                                              POE Loop (P→O→E)            │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## RPC 接口设计

### poe.prepare - 生成契约

```typescript
// Request
{
  "method": "poe.prepare",
  "params": {
    "instruction": "帮我重构 auth.rs，把硬编码的 key 换成环境变量",
    "context": {                          // 可选：额外上下文
      "working_dir": "/workspace/myproject",
      "files": ["src/auth.rs"]            // 提示 Builder 关注的文件
    }
  }
}

// Response
{
  "contract_id": "contract-a1b2c3d4",
  "manifest": {
    "task_id": "refactor-auth-env-var",
    "objective": "重构 Auth 模块，将硬编码密钥替换为环境变量",
    "hard_constraints": [
      { "type": "FileExists", "params": { "path": "src/auth.rs" } },
      { "type": "FileNotContains", "params": { "path": "src/auth.rs", "pattern": "sk-[a-zA-Z0-9]+" } },
      { "type": "CommandPasses", "params": { "cmd": "cargo", "args": ["check"] } }
    ],
    "soft_metrics": [
      {
        "rule": { "type": "SemanticCheck", "params": { ... } },
        "weight": 0.8,
        "threshold": 0.7
      }
    ],
    "max_attempts": 5
  },
  "created_at": "2026-02-02T10:30:00Z",
  "instruction": "帮我重构 auth.rs..."   // 回显原始指令
}
```

### poe.sign - 签署契约

```typescript
// Request
{
  "method": "poe.sign",
  "params": {
    "contract_id": "contract-a1b2c3d4",
    "amendments": "还要确保通过 cargo clippy 检查",  // 可选：自然语言修改
    "manifest_override": { ... },                    // 可选：JSON 覆盖
    "stream": true
  }
}

// Response
{
  "task_id": "refactor-auth-env-var",
  "session_key": "agent:main:poe:refactor-auth-env-var",
  "signed_at": "2026-02-02T10:32:00Z",
  "final_manifest": { ... }   // 合并修改后的最终契约
}
```

### poe.reject - 拒绝契约

```typescript
// Request
{
  "method": "poe.reject",
  "params": {
    "contract_id": "contract-a1b2c3d4",
    "reason": "不需要了"                  // 可选
  }
}

// Response
{
  "contract_id": "contract-a1b2c3d4",
  "rejected": true
}
```

### poe.pending - 查询待签署契约

```typescript
// Request
{ "method": "poe.pending", "params": {} }

// Response
{
  "contracts": [
    {
      "contract_id": "contract-a1b2c3d4",
      "instruction": "帮我重构 auth.rs...",
      "objective": "重构 Auth 模块...",
      "created_at": "2026-02-02T10:30:00Z"
    }
  ],
  "count": 1
}
```

---

## 核心数据结构

### PendingContract

```rust
// core/src/poe/contract.rs

/// 待签署的 POE 契约
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingContract {
    /// 契约唯一标识
    pub contract_id: String,

    /// 原始用户指令
    pub instruction: String,

    /// 生成的成功契约
    pub manifest: SuccessManifest,

    /// 可选的上下文信息
    pub context: Option<ContractContext>,

    /// 创建时间
    pub created_at: DateTime<Utc>,
}

/// 契约生成时的上下文
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractContext {
    /// 工作目录
    pub working_dir: Option<String>,

    /// 相关文件列表
    pub files: Vec<String>,

    /// 会话信息
    pub session_key: Option<String>,
}

/// 签署请求
#[derive(Debug, Clone, Deserialize)]
pub struct SignRequest {
    /// 契约 ID
    pub contract_id: String,

    /// 自然语言修改
    #[serde(default)]
    pub amendments: Option<String>,

    /// JSON 直接覆盖（高级模式）
    #[serde(default)]
    pub manifest_override: Option<SuccessManifest>,

    /// 是否流式返回事件
    #[serde(default = "default_true")]
    pub stream: bool,
}
```

### PendingContractStore

```rust
// core/src/poe/contract_store.rs

/// 待签署契约的内存存储
pub struct PendingContractStore {
    contracts: Arc<RwLock<HashMap<String, PendingContract>>>,
}

impl PendingContractStore {
    pub fn new() -> Self;

    /// 存入新契约
    pub async fn insert(&self, contract: PendingContract);

    /// 取出契约（签署时调用，移除并返回）
    pub async fn take(&self, contract_id: &str) -> Option<PendingContract>;

    /// 查询契约（不移除）
    pub async fn get(&self, contract_id: &str) -> Option<PendingContract>;

    /// 移除契约（拒绝时调用）
    pub async fn remove(&self, contract_id: &str) -> bool;

    /// 列出所有待签署契约
    pub async fn list(&self) -> Vec<PendingContract>;

    /// 清理所有待签署契约
    pub async fn clear(&self) -> usize;
}
```

---

## ManifestBuilder 增量模式

```rust
// core/src/poe/manifest.rs

impl ManifestBuilder {
    /// 增量修改现有契约
    ///
    /// 接收用户的自然语言修改指令，合并到现有 manifest 中。
    pub async fn amend(
        &self,
        current: &SuccessManifest,
        amendment: &str,
    ) -> Result<SuccessManifest> {
        // 1. 序列化当前 manifest
        // 2. 构建包含原 manifest + 修改指令的 prompt
        // 3. 调用 LLM 生成更新后的完整 manifest
        // 4. 解析并返回
    }

    /// 合并 JSON 覆盖（高级模式）
    ///
    /// 约束采用追加策略（而非覆盖）
    pub fn merge_override(
        current: &SuccessManifest,
        override_manifest: &SuccessManifest,
    ) -> SuccessManifest {
        // 追加 hard_constraints 和 soft_metrics
        // 其他字段按优先级覆盖
    }
}
```

**设计决策**：
- `amend()` 使用 LLM 理解自然语言修改意图
- `merge_override()` 纯 Rust 逻辑，约束采用**追加**而非覆盖
- 如需删除约束，必须通过 `amend()` 明确指示

---

## Gateway Handler 实现

```rust
// core/src/gateway/handlers/poe.rs

/// POE 契约服务
pub struct PoeContractService<W: Worker + 'static> {
    manifest_builder: Arc<ManifestBuilder>,
    contract_store: Arc<PendingContractStore>,
    run_manager: Arc<PoeRunManager<W>>,
}

impl<W: Worker + 'static> PoeContractService<W> {
    /// poe.prepare - 生成契约
    pub async fn prepare(&self, params: PrepareParams) -> Result<PrepareResult>;

    /// poe.sign - 签署契约并执行
    pub async fn sign(&self, params: SignRequest) -> Result<SignResult>;

    /// poe.reject - 拒绝契约
    pub async fn reject(&self, params: RejectParams) -> Result<RejectResult>;

    /// poe.pending - 列出待签署契约
    pub async fn pending(&self) -> Result<PendingResult>;
}
```

---

## 事件流设计

### 阶段 1: 契约生成

```json
{
    "method": "poe.contract_generated",
    "params": {
        "data": {
            "contract_id": "contract-a1b2c3d4",
            "objective": "重构 Auth 模块",
            "constraint_count": 3,
            "metric_count": 1
        }
    }
}
```

### 阶段 2: 签署后执行

```json
// 签署确认
{ "method": "poe.signed", "params": { "data": { "contract_id": "...", "task_id": "...", "amendments_applied": true } } }

// POE 循环
{ "method": "poe.step", "params": { "data": { "task_id": "...", "attempt": 1, "phase": "operation" } } }

// 验证结果
{ "method": "poe.validation", "params": { "data": { "task_id": "...", "passed": false, "distance_score": 0.3 } } }

// 完成
{ "method": "poe.completed", "params": { "data": { "task_id": "...", "outcome": { "outcome": "Success" } } } }
```

### 阶段 3: 异常情况

```json
// 拒绝
{ "method": "poe.rejected", "params": { "data": { "contract_id": "...", "reason": "用户取消" } } }

// 策略切换
{ "method": "poe.strategy_switch", "params": { "data": { "task_id": "...", "reason": "连续 3 次尝试无进展" } } }
```

---

## 渐进信任预留接口

```rust
// core/src/poe/trust.rs

/// 自动放行决策
pub enum AutoApprovalDecision {
    RequireSignature { reason: String },
    AutoApprove { reason: String, confidence: f32 },
}

/// 信任评估器 trait
pub trait TrustEvaluator: Send + Sync {
    fn evaluate(&self, manifest: &SuccessManifest, context: &TrustContext) -> AutoApprovalDecision;
}

// V1.0: 始终要求签署
pub struct AlwaysRequireSignature;

// V1.5: 白名单规则（预留）
pub struct WhitelistTrustEvaluator { ... }

// V2.0: 经验信任评估（预留）
pub struct ExperienceTrustEvaluator { ... }
```

### 演进路径

| 版本 | 策略 | 说明 |
|------|------|------|
| V1.0 | 强制审批 | 所有契约需要用户签署 |
| V1.5 | 白名单 | 低风险任务（仅 FileExists，无删除）自动放行 |
| V2.0 | 信任分 | 已结晶技能（成功率 > 95%，执行次数 >= 5）自动放行 |

---

## 实现计划

### Phase 1: 核心数据结构

```
新增文件:
  [+] core/src/poe/contract.rs        (~80 行)
  [+] core/src/poe/contract_store.rs  (~60 行)
  [+] core/src/poe/trust.rs           (~120 行)
```

### Phase 2: ManifestBuilder 扩展

```
修改文件:
  [M] core/src/poe/manifest.rs        (+60 行: amend, merge_override)
```

### Phase 3: Gateway Handler

```
修改文件:
  [M] core/src/poe/mod.rs             (+3 行: 导出新模块)
  [M] core/src/gateway/handlers/poe.rs (+200 行: 新处理器)
```

### Phase 4: Router 注册

```
修改文件:
  [M] core/src/gateway/router.rs      (+10 行: 注册 RPC)
```

---

## 测试计划

### 单元测试

- `contract_store`: insert/take/remove/list/clear
- `manifest.amend()`: 自然语言修改解析
- `manifest.merge_override()`: JSON 合并逻辑
- `trust`: 各 TrustEvaluator 实现

### 集成测试

- prepare → sign → execute 完整流程
- prepare → reject 流程
- prepare → amend → sign 流程
- 并发签署同一契约（应失败）

---

## 验收标准

- [ ] `poe.prepare` 可从自然语言指令生成 SuccessManifest
- [ ] `poe.sign` 支持无修改签署
- [ ] `poe.sign` 支持自然语言修改 (amendments)
- [ ] `poe.sign` 支持 JSON 覆盖 (manifest_override)
- [ ] `poe.reject` 正确清理待签署契约
- [ ] `poe.pending` 返回所有待签署契约
- [ ] 签署后复用现有 PoeRunManager 执行 POE 循环
- [ ] 事件流正确推送 signed/rejected/step/validation/completed
- [ ] `cargo test` 全部通过
- [ ] `cargo clippy` 无警告
