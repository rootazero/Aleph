# Skill Sandboxing Phase 3: User Approval Workflow & Security Testing

> **Date**: 2026-02-09
> **Status**: Design Complete
> **Phase**: Phase 3 - User Approval Workflow and Transparent Audit

---

## Executive Summary

### 核心目标

Phase 3 在 Phase 2（沙箱集成）的基础上，建立**契约式权限审批系统**和**透明化审计机制**，实现"安全性"与"用户体验"的平衡。

### 设计理念

**从"许可"到"契约"**：用户在工具生成时审批 `tool_definition.json`，本质上是签署一份安全契约（Security Contract），明确工具的"行为边界"（Capabilities）和"参数意图"（Parameter Bindings）。

**Adaptive Runtime Escorts**：系统采用"静默执行 + 异常触发"模式：
- **静默执行**：当运行时参数完全落在已批准的 Manifest 范围内时，无需打断用户
- **异常触发**：当参数超出范围、访问敏感目录或使用未声明绑定时，升级到用户确认

**三阶段信任演进**：
1. **Draft 阶段**（Generation）：用户审核契约（Manifest）
2. **Trial 阶段**（First Use）：首次执行时展示"权限实况预览"
3. **Verified 阶段**（Subsequent）：参数模式未变时静默执行

### 架构原则

- **统一审批体验**：扩展现有 `ApprovalManager`，统一处理 shell 命令和 skill 权限审批
- **混合存储模式**：`tool_definition.json` 存储契约状态，SQLite 存储审计历史
- **权限透明度优先**：Audit Dashboard 提供双视图（工具风险 + 执行历史）

---

## 核心架构

### 统一审批系统架构

扩展现有 `ApprovalManager` 支持两种审批类型：

```rust
// core/src/exec/approval/types.rs
pub enum ApprovalRequest {
    Command(CommandApprovalRequest),
    Capability(CapabilityApprovalRequest),
}

pub struct CapabilityApprovalRequest {
    pub tool_name: String,
    pub tool_description: String,
    pub required_capabilities: RequiredCapabilities,
    pub resolved_capabilities: Capabilities,  // 完整解析后的权限
    pub trust_stage: TrustStage,
    pub generation_context: GenerationContext,  // AI 生成的上下文
}

pub enum TrustStage {
    Draft,      // 工具刚生成，等待首次审批
    Trial,      // 已审批，等待首次执行确认
    Verified,   // 已执行多次，进入静默模式
}
```

### Adaptive Runtime Escorts 机制

**静默执行条件**（无需用户确认）：
1. 工具已获得 Manifest 授权（`approval_metadata.approved = true`）
2. 运行时参数完全落在 Manifest 声明的范围内
3. 路径不属于敏感目录（`.ssh`, `.gnupg`, `Keychain.app`）
4. 信任阶段为 `Verified`

**触发式确认条件**（Escalate to User）：
1. 参数超出 `custom_paths` 范围
2. 访问敏感目录
3. 使用未声明的参数绑定
4. 信任阶段为 `Trial`（首次执行）

```rust
pub struct EscalationTrigger {
    pub reason: EscalationReason,
    pub requested_path: Option<PathBuf>,
    pub approved_paths: Vec<String>,
}

pub enum EscalationReason {
    PathOutOfScope,
    SensitiveDirectory,
    UndeclaredBinding,
    FirstExecution,
}
```

---

## 数据结构与存储

### 混合存储模式

**tool_definition.json（契约文本）**
```json
{
  "name": "log_analyzer",
  "description": "Analyze system logs",
  "required_capabilities": {
    "preset": "file_processor",
    "overrides": {
      "file_read": ["/var/log/*"]
    },
    "parameter_bindings": [...]
  },
  "approval_metadata": {
    "approved": true,
    "approved_at": "2026-02-09T10:30:00Z",
    "approved_by": "owner",
    "approval_scope": "permanent",
    "trust_stage": "verified",
    "execution_count": 42,
    "last_executed_at": "2026-02-09T15:20:00Z"
  }
}
```

**SQLite 审计表（审计日志）**
```sql
-- 权限审批历史
CREATE TABLE capability_approvals (
    id INTEGER PRIMARY KEY,
    tool_name TEXT NOT NULL,
    capabilities_hash TEXT NOT NULL,  -- 权限内容的 hash
    approved BOOLEAN NOT NULL,
    approved_by TEXT NOT NULL,
    approval_scope TEXT NOT NULL,  -- once/session/permanent
    approved_at INTEGER NOT NULL,
    reason TEXT
);

-- 运行时 Escalation 记录
CREATE TABLE capability_escalations (
    id INTEGER PRIMARY KEY,
    tool_name TEXT NOT NULL,
    execution_id TEXT NOT NULL,
    escalation_reason TEXT NOT NULL,  -- path_out_of_scope/sensitive_dir/...
    requested_path TEXT,
    approved_paths TEXT,  -- JSON array
    user_decision TEXT,  -- approved/denied
    decided_at INTEGER NOT NULL
);

-- 工具执行审计（扩展现有 audit 表）
ALTER TABLE audit_log ADD COLUMN tool_execution_context TEXT;  -- JSON
```

---

## 审批工作流

### 工具生成时审批流程

```
ToolGenerator → GeneratedToolDefinition
                        ↓
                CapabilityResolver (解析完整权限)
                        ↓
                ApprovalManager.request_capability_approval()
                        ↓
                ApprovalBridge (发送到客户端 UI)
                        ↓
                User Decision (Approve/Deny/Modify)
                        ↓
                Save to tool_definition.json + SQLite
```

### 运行时 Escalation 流程

```
SandboxedToolExecutor.execute()
                        ↓
        检查 approval_metadata.approved?
                ↓ (Yes)
        检查 trust_stage?
                ↓
        ┌───────┴───────┐
        │               │
    Trial           Verified
        │               │
        ↓               ↓
    首次执行确认    参数范围检查
        │               │
        ↓               ↓
    展示权限预览    超出范围?
        │               │
        ↓               ↓ (Yes)
    用户确认        触发 Escalation
        │               │
        └───────┬───────┘
                ↓
        记录到 capability_escalations 表
                ↓
        执行或拒绝
```

### ApprovalManager 扩展

```rust
impl ApprovalManager {
    pub async fn request_capability_approval(
        &self,
        request: CapabilityApprovalRequest,
    ) -> Result<ApprovalDecision> {
        // 1. 检查是否已审批（通过 capabilities_hash）
        if let Some(existing) = self.storage.get_capability_approval(
            &request.tool_name,
            &request.capabilities_hash(),
        ) {
            return Ok(existing);
        }

        // 2. 发送审批请求到客户端
        let decision = self.bridge.request_capability_approval(request).await?;

        // 3. 保存审批决策
        self.storage.save_capability_approval(&decision)?;

        Ok(decision)
    }

    pub async fn check_runtime_escalation(
        &self,
        tool_name: &str,
        runtime_params: &HashMap<String, String>,
        approved_capabilities: &Capabilities,
    ) -> Result<Option<EscalationTrigger>> {
        // 检查是否需要触发 escalation
        // 返回 None 表示静默执行，Some(trigger) 表示需要用户确认
    }
}
```

---

## Audit Dashboard

### 双视图模式

**视图 1: 工具风险视图（默认）**

展示所有工具的风险评分和状态：

```rust
pub struct ToolRiskSummary {
    pub tool_name: String,
    pub trust_stage: TrustStage,
    pub capabilities: Capabilities,
    pub execution_count: u32,
    pub success_rate: f32,
    pub escalation_count: u32,
    pub last_executed_at: Option<DateTime<Utc>>,
    pub risk_score: RiskScore,  // Low/Medium/High
}

pub enum RiskScore {
    Low,     // 只读文件，无网络，无 exec
    Medium,  // 写文件，或有限网络访问
    High,    // exec 权限，或访问敏感目录
}
```

**视图 2: 执行历史视图（点击工具展开）**

展示特定工具的所有执行记录：

```rust
pub struct ToolExecutionRecord {
    pub execution_id: String,
    pub executed_at: DateTime<Utc>,
    pub status: ExecutionStatus,  // Success/Failed/Denied
    pub runtime_params: HashMap<String, String>,
    pub resolved_capabilities: Capabilities,
    pub escalation: Option<EscalationRecord>,
    pub exit_code: Option<i32>,
}

pub struct EscalationRecord {
    pub reason: EscalationReason,
    pub requested_path: Option<PathBuf>,
    pub user_decision: UserDecision,  // Approved/Denied
    pub decided_at: DateTime<Utc>,
}
```

### CLI 命令接口

```bash
# 查看所有工具的风险评分
aleph audit tools --sort-by risk

# 查看特定工具的执行历史
aleph audit tool log_analyzer --limit 20

# 查看所有 escalation 记录
aleph audit escalations --since 7d

# 查看权限对比（Manifest vs 实际使用）
aleph audit tool log_analyzer --compare-capabilities
```

---

## 安全与性能测试

### 安全测试场景

**测试目标**：验证沙箱隔离效果和审批系统的防御能力

**测试用例 1: 恶意路径遍历**
```rust
#[tokio::test]
async fn test_malicious_path_traversal() {
    // 工具声明只能访问 /tmp/workspace/*
    // 但运行时尝试访问 /tmp/workspace/../../etc/passwd
    // 预期：触发 EscalationReason::PathOutOfScope
}
```

**测试用例 2: 敏感目录访问**
```rust
#[tokio::test]
async fn test_sensitive_directory_access() {
    // 工具声明可以读取 ~/Documents/*
    // 但运行时尝试访问 ~/.ssh/id_rsa
    // 预期：触发 EscalationReason::SensitiveDirectory
}
```

**测试用例 3: 未声明参数绑定**
```rust
#[tokio::test]
async fn test_undeclared_parameter_binding() {
    // 工具只声明了 input_file 参数绑定
    // 但运行时尝试使用 output_file 参数
    // 预期：触发 EscalationReason::UndeclaredBinding
}
```

**测试用例 4: 权限升级攻击**
```rust
#[tokio::test]
async fn test_privilege_escalation() {
    // 工具声明 allow_exec = false
    // 但运行时尝试执行 subprocess
    // 预期：沙箱拒绝执行，记录到 audit_log
}
```

### 性能测试场景

**测试目标**：评估沙箱和审批系统的性能开销

**基准测试 1: 沙箱启动开销**
- 测量从 `SandboxedToolExecutor.execute()` 到实际执行的延迟
- 目标：< 50ms

**基准测试 2: 参数范围检查开销**
- 测量 `check_runtime_escalation()` 的执行时间
- 目标：< 10ms

**基准测试 3: 审计日志写入开销**
- 测量 SQLite 写入 `capability_escalations` 表的时间
- 目标：< 5ms（异步写入）

---

## 实施计划

### 实施优先级（按顺序）

**阶段 1: 核心审批系统（2 周）**
1. 扩展 `ApprovalManager` 支持 `CapabilityApprovalRequest`
2. 实现 `TrustStage` 状态机（Draft → Trial → Verified）
3. 实现 `check_runtime_escalation()` 逻辑
4. 添加 `approval_metadata` 到 `tool_definition.json`
5. 创建 SQLite 审计表（`capability_approvals`, `capability_escalations`）

**阶段 2: CLI 审批接口（1 周）**
1. 实现 CLI 审批提示（类似现有的 shell 命令审批）
2. 展示 Capabilities 列表和参数绑定预览
3. 支持 ApprovalScope（Once/Session/Permanent）选择
4. 实现首次执行确认（Trial 阶段）

**阶段 3: Audit Dashboard 数据层（1 周）**
1. 实现 `ToolRiskSummary` 查询逻辑
2. 实现 `ToolExecutionRecord` 查询逻辑
3. 计算 `RiskScore`（基于 Capabilities）
4. 实现 CLI 命令（`aleph audit tools/tool/escalations`）

**阶段 4: 安全测试（1 周）**
1. 编写恶意路径遍历测试
2. 编写敏感目录访问测试
3. 编写未声明参数绑定测试
4. 编写权限升级攻击测试
5. 验证所有测试通过

**阶段 5: 性能测试与优化（1 周）**
1. 实现性能基准测试
2. 优化 `check_runtime_escalation()` 性能
3. 优化审计日志异步写入
4. 验证性能目标达成

**阶段 6: UI 集成（后续迭代）**
- macOS App 审批 UI
- Desktop App 审批 UI
- Audit Dashboard 可视化界面

### 总工期估算
- **核心功能**：5 周（阶段 1-5）
- **UI 集成**：3 周（可并行或后置）

---

## 成功标准

### 功能完整性
- ✅ 工具生成时可以触发权限审批
- ✅ 支持 ApprovalScope（Once/Session/Permanent）
- ✅ Trial 阶段首次执行展示权限预览
- ✅ Verified 阶段参数超出范围时触发 escalation
- ✅ 所有审批决策记录到 SQLite
- ✅ CLI 命令可以查询工具风险和执行历史

### 安全性
- ✅ 恶意路径遍历被拦截
- ✅ 敏感目录访问触发 escalation
- ✅ 未声明参数绑定被拒绝
- ✅ 权限升级攻击被沙箱阻止

### 性能
- ✅ 沙箱启动开销 < 50ms
- ✅ 参数范围检查 < 10ms
- ✅ 审计日志写入 < 5ms（异步）

### 用户体验
- ✅ 审批提示清晰展示 Capabilities 和参数绑定
- ✅ Verified 阶段工具静默执行，无打断
- ✅ Escalation 提示明确说明原因和请求的权限

---

## 风险评估

### 高风险
- **风险**：`check_runtime_escalation()` 逻辑复杂，可能存在绕过漏洞
- **缓解**：编写全面的安全测试，覆盖各种攻击场景

### 中风险
- **风险**：混合存储模式可能导致 `tool_definition.json` 和 SQLite 数据不一致
- **缓解**：使用 `capabilities_hash` 检测权限变更，强制重新审批

### 低风险
- **风险**：性能开销可能影响工具执行速度
- **缓解**：异步写入审计日志，优化参数范围检查算法

---

## 后续阶段

- **Phase 4**: UI 集成（macOS/Desktop App 审批界面和 Audit Dashboard 可视化）
- **Phase 5**: 跨平台支持（Linux seccomp-bpf + AppArmor/SELinux，Windows Job Objects + AppContainer）
- **Phase 6**: 高级特性（ML 推理、动态调整、异常检测）

---

**Design Status**: Complete
**Ready for Implementation**: Yes
**Estimated Effort**: 5 weeks (core) + 3 weeks (UI)
**Risk Level**: Medium
