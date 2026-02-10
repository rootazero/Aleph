# Collaborative Skill Evolution: 协作式技能进化架构设计

> **核心理念**：用户作为"合规官"而非"程序员"，LLM 作为"执行者"而非"建议者"

**日期**：2026-02-11
**状态**：设计阶段
**作者**：Claude Sonnet 4.5

---

## 1. 愿景：80% 软件消失的世界

### 1.1 核心洞察

如果赋予 AI 足够的权限，80% 的软件都会消失。绝大部分软件能干的工作，脚本都可以完成。

**传统软件的问题**：
- 复杂的 GUI 界面
- 固定的功能边界
- 需要用户学习如何使用

**Aleph + Skills 的解决方案**：
- LLM 理解用户意图
- 动态生成脚本执行
- 通过 Skills 约束和指导 LLM

### 1.2 混合架构（Hybrid Architecture）

```
┌─────────────────────────────────────────────────────────────┐
│                    Skill: 个人财务审计                       │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  语义层 (Semantic Layer)                                    │
│  ┌────────────────────────────────────────────────────┐    │
│  │ SUCCESS_MANIFEST.md                                 │    │
│  │ ─────────────────────                               │    │
│  │ ## 目标                                             │    │
│  │ 处理个人财务报表，生成审计报告                      │    │
│  │                                                     │    │
│  │ ## 允许的操作                                       │    │
│  │ - 读取 PDF 格式的银行对账单                         │    │
│  │ - 读取 CSV 格式的信用卡账单                         │    │
│  │ - 写入 Excel 格式的审计报告                         │    │
│  │ - 使用本地 Python 脚本进行数据分析                  │    │
│  │                                                     │    │
│  │ ## 禁止的操作                                       │    │
│  │ - 将任何财务数据发送到外部网络                      │    │
│  │ - 修改原始财务报表文件                              │    │
│  │ - 执行任何网络请求                                  │    │
│  │ - 访问系统目录或其他用户文件                        │    │
│  │                                                     │    │
│  │ ## 推荐的工具组合                                   │    │
│  │ 1. pdf_reader: 读取 PDF 银行对账单                 │    │
│  │ 2. csv_parser: 解析 CSV 信用卡账单                 │    │
│  │ 3. data_analyzer: 本地数据分析和计算               │    │
│  │ 4. excel_writer: 生成审计报告                      │    │
│  └────────────────────────────────────────────────────┘    │
│                           ↓                                  │
│                    LLM 理解并遵守                            │
│                           ↓                                  │
│  执行层 (Execution Layer)                                   │
│  ┌────────────────────────────────────────────────────┐    │
│  │ Scripts & Tools                                     │    │
│  │ ─────────────────                                   │    │
│  │ • pdf_reader.py: PyPDF2 解析 PDF                   │    │
│  │ • csv_parser.py: pandas 解析 CSV                   │    │
│  │ • data_analyzer.py: 计算总和、分类、趋势           │    │
│  │ • excel_writer.py: openpyxl 生成报告               │    │
│  └────────────────────────────────────────────────────┘    │
│                           ↓                                  │
│  约束层 (Constraint Layer)                                  │
│  ┌────────────────────────────────────────────────────┐    │
│  │ Capabilities (硬约束)                               │    │
│  │ ─────────────────────                               │    │
│  │ filesystem: [                                       │    │
│  │   ReadOnly { path: "/Users/*/Documents/Finance" },  │    │
│  │   ReadWrite { path: "/Users/*/Documents/Audit" },   │    │
│  │   TempWorkspace,                                    │    │
│  │ ],                                                  │    │
│  │ network: Deny,  // 强制禁止网络                     │    │
│  │ process: {                                          │    │
│  │   no_fork: true,                                    │    │
│  │   max_execution_time: 300,  // 5 分钟              │    │
│  │   max_memory_mb: 512,                               │    │
│  │ },                                                  │    │
│  │ environment: Restricted,                            │    │
│  └────────────────────────────────────────────────────┘    │
│                           ↓                                  │
│                    macOS Sandbox 强制执行                    │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

---

## 2. 协作式进化流程（Collaborative Evolution）

### 2.1 四阶段闭环

```
┌─────────────────────────────────────────────────────────────┐
│                  协作式进化生命周期                          │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  Phase 1: 探索期 (Exploration)                              │
│  ┌────────────────────────────────────────────────────┐    │
│  │ • LLM 在极度受限的通用沙箱中运行临时脚本           │    │
│  │ • 沙箱配置: data_transformer preset                │    │
│  │   - filesystem: [TempWorkspace]                    │    │
│  │   - network: Deny                                  │    │
│  │   - process: { no_fork: true, max_time: 60s }     │    │
│  │ • 用户指令: "帮我分析这个月的银行对账单"           │    │
│  │ • LLM 尝试: 写 Python 脚本读取 PDF → 失败         │    │
│  │ • LLM 请求: "需要读取 /Documents/Finance/*.pdf"    │    │
│  └────────────────────────────────────────────────────┘    │
│                           ↓                                  │
│  Phase 2: 提议期 (Crystallization Request)                  │
│  ┌────────────────────────────────────────────────────┐    │
│  │ • SolidificationPipeline 检测成功模式               │    │
│  │ • LLM 生成 Skill 提议:                              │    │
│  │   1. SUCCESS_MANIFEST.md (软约束)                  │    │
│  │   2. Capabilities (硬约束建议)                     │    │
│  │ • 自校验 (Constraint Validator):                   │    │
│  │   - 检查软约束与硬约束是否匹配                     │    │
│  │   - 例如: Manifest 说"禁止网络"，Config 必须      │    │
│  │     network: Deny                                  │    │
│  │   - 如果不匹配，拦截提议并要求 LLM 修正            │    │
│  └────────────────────────────────────────────────────┘    │
│                           ↓                                  │
│  Phase 3: 评审期 (Human Sign-off)                           │
│  ┌────────────────────────────────────────────────────┐    │
│  │ • 用户看到语义化的软约束 (SUCCESS_MANIFEST.md)     │    │
│  │ • UI 展示:                                          │    │
│  │   ┌──────────────────────────────────────────┐    │    │
│  │   │ 新技能提议: 个人财务审计                 │    │    │
│  │   │                                          │    │    │
│  │   │ 目标: 处理个人财务报表，生成审计报告     │    │    │
│  │   │                                          │    │    │
│  │   │ 允许:                                    │    │    │
│  │   │ ✓ 读取 /Documents/Finance/*.pdf          │    │    │
│  │   │ ✓ 写入 /Documents/Audit/*.xlsx           │    │    │
│  │   │ ✓ 使用本地 Python 脚本                   │    │    │
│  │   │                                          │    │    │
│  │   │ 禁止:                                    │    │    │
│  │   │ ✗ 网络访问                               │    │    │
│  │   │ ✗ 修改原始文件                           │    │    │
│  │   │ ✗ 访问系统目录                           │    │    │
│  │   │                                          │    │    │
│  │   │ [批准] [拒绝] [修改]                     │    │    │
│  │   └──────────────────────────────────────────┘    │    │
│  │ • 用户点击"批准"，硬约束随即生效                   │    │
│  └────────────────────────────────────────────────────┘    │
│                           ↓                                  │
│  Phase 4: 成熟期 (Promotion)                                │
│  ┌────────────────────────────────────────────────────┐    │
│  │ • Skill 获得"常驻特权"                              │    │
│  │ • 正式入驻 LLM 的技能中心                           │    │
│  │ • 后续执行:                                         │    │
│  │   - LLM 识别到"财务审计"意图                       │    │
│  │   - 自动激活该 Skill 的上下文                      │    │
│  │   - 在 Skill 的约束下执行脚本                      │    │
│  │ • 持续优化:                                         │    │
│  │   - 记录成功/失败案例                              │    │
│  │   - 自动调整推荐的工具组合                         │    │
│  └────────────────────────────────────────────────────┘    │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 约束自动推导（Constraint Derivation）

**核心机制**：LLM 同时生成软约束和硬约束，系统自动校验两者是否匹配。

#### 2.2.1 Constraint Validator

```rust
pub struct ConstraintValidator;

impl ConstraintValidator {
    /// 校验软约束与硬约束是否匹配
    pub fn validate(
        manifest: &SuccessManifest,
        capabilities: &Capabilities,
    ) -> Result<ValidationReport, ConstraintMismatch> {
        let mut report = ValidationReport::new();

        // 规则 1: 如果 Manifest 禁止网络，Capabilities 必须 network: Deny
        if manifest.prohibits_network() && !capabilities.network.is_deny() {
            report.add_error(ConstraintMismatch::NetworkMismatch {
                manifest: "禁止网络访问",
                capabilities: format!("{:?}", capabilities.network),
            });
        }

        // 规则 2: 如果 Manifest 允许读取路径，Capabilities 必须包含对应的 ReadOnly
        for allowed_path in manifest.allowed_read_paths() {
            if !capabilities.filesystem.contains_read_access(&allowed_path) {
                report.add_error(ConstraintMismatch::FileSystemMismatch {
                    manifest_path: allowed_path,
                    reason: "Manifest 允许读取，但 Capabilities 未授权",
                });
            }
        }

        // 规则 3: 如果 Capabilities 授予了权限，Manifest 必须明确允许
        for fs_cap in &capabilities.filesystem {
            match fs_cap {
                FileSystemCapability::ReadWrite { path } => {
                    if !manifest.allows_write_to(path) {
                        report.add_error(ConstraintMismatch::UnauthorizedWrite {
                            path: path.clone(),
                            reason: "Capabilities 授予写权限，但 Manifest 未明确允许",
                        });
                    }
                }
                _ => {}
            }
        }

        if report.has_errors() {
            Err(ConstraintMismatch::ValidationFailed(report))
        } else {
            Ok(report)
        }
    }
}
```

#### 2.2.2 SUCCESS_MANIFEST.md 格式

```markdown
# Skill: 个人财务审计

## Metadata
- **skill_id**: `personal_finance_audit`
- **version**: `1.0.0`
- **created_at**: `2026-02-11T10:00:00Z`
- **author**: `llm-generated`

## 目标 (Goal)
处理个人财务报表（银行对账单、信用卡账单），生成审计报告，帮助用户了解财务状况。

## 允许的操作 (Allowed Operations)

### 文件系统
- **读取**: `/Users/*/Documents/Finance/**/*.pdf` (银行对账单)
- **读取**: `/Users/*/Documents/Finance/**/*.csv` (信用卡账单)
- **写入**: `/Users/*/Documents/Audit/**/*.xlsx` (审计报告)
- **临时工作区**: 允许使用临时目录进行中间处理

### 脚本执行
- **Python**: 允许执行本地 Python 脚本
- **库**: `PyPDF2`, `pandas`, `openpyxl`, `numpy`

### 数据处理
- **解析**: PDF, CSV 格式
- **计算**: 总和、平均值、分类统计
- **生成**: Excel 报告

## 禁止的操作 (Prohibited Operations)

### 网络
- **严禁**: 任何形式的网络请求
- **理由**: 财务数据属于敏感信息，不得外泄

### 文件系统
- **严禁**: 修改原始财务报表文件
- **严禁**: 访问系统目录 (`/System`, `/usr`, `/etc`)
- **严禁**: 访问其他用户的文件

### 进程
- **严禁**: fork/exec 子进程
- **严禁**: 执行系统命令（除了 Python 解释器）

## 推荐的工具组合 (Recommended Tool Chain)

1. **pdf_reader**: 使用 PyPDF2 读取 PDF 银行对账单
2. **csv_parser**: 使用 pandas 解析 CSV 信用卡账单
3. **data_analyzer**: 计算总和、分类、趋势分析
4. **excel_writer**: 使用 openpyxl 生成审计报告

## 成功标准 (Success Criteria)

- 生成的审计报告包含所有交易记录
- 计算的总和与原始数据一致
- 报告格式清晰，易于阅读
- 执行时间 < 5 分钟
- 内存使用 < 512MB

## 失败处理 (Failure Handling)

- 如果 PDF 解析失败，尝试 OCR
- 如果 CSV 格式不标准，提示用户检查文件
- 如果内存不足，分批处理数据

## 安全保证 (Security Guarantees)

- 所有财务数据仅在本地处理
- 不会发送任何数据到外部网络
- 原始文件不会被修改
- 临时文件在处理完成后自动删除
```

---

## 3. 原型实现：个人财务审计 Skill

### 3.1 目录结构

```
~/.aleph/skills/personal_finance_audit/
├── SUCCESS_MANIFEST.md          # 软约束（语义层）
├── capabilities.json            # 硬约束（沙箱配置）
├── tools/
│   ├── pdf_reader.py            # PDF 解析工具
│   ├── csv_parser.py            # CSV 解析工具
│   ├── data_analyzer.py         # 数据分析工具
│   └── excel_writer.py          # Excel 生成工具
├── examples/
│   ├── sample_bank_statement.pdf
│   └── sample_credit_card.csv
└── tests/
    └── test_skill.py            # 集成测试
```

### 3.2 capabilities.json

```json
{
  "filesystem": [
    {
      "type": "read_only",
      "path": "/Users/*/Documents/Finance"
    },
    {
      "type": "read_write",
      "path": "/Users/*/Documents/Audit"
    },
    {
      "type": "temp_workspace"
    }
  ],
  "network": "deny",
  "process": {
    "no_fork": true,
    "max_execution_time": 300,
    "max_memory_mb": 512
  },
  "environment": "restricted"
}
```

### 3.3 工具实现示例

#### pdf_reader.py

```python
#!/usr/bin/env python3
"""
PDF 银行对账单解析工具
"""
import sys
import json
from PyPDF2 import PdfReader

def parse_bank_statement(pdf_path: str) -> dict:
    """解析银行对账单 PDF"""
    reader = PdfReader(pdf_path)
    transactions = []

    for page in reader.pages:
        text = page.extract_text()
        # 解析交易记录（简化示例）
        lines = text.split('\n')
        for line in lines:
            if is_transaction_line(line):
                transaction = parse_transaction(line)
                transactions.append(transaction)

    return {
        "source": pdf_path,
        "transactions": transactions,
        "total": sum(t["amount"] for t in transactions)
    }

def is_transaction_line(line: str) -> bool:
    """判断是否为交易记录行"""
    # 简化示例：检查是否包含日期和金额
    return bool(re.match(r'\d{4}-\d{2}-\d{2}.*\$\d+\.\d{2}', line))

def parse_transaction(line: str) -> dict:
    """解析单条交易记录"""
    # 简化示例：提取日期、描述、金额
    parts = line.split()
    return {
        "date": parts[0],
        "description": " ".join(parts[1:-1]),
        "amount": float(parts[-1].replace('$', ''))
    }

if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("Usage: pdf_reader.py <pdf_path>")
        sys.exit(1)

    result = parse_bank_statement(sys.argv[1])
    print(json.dumps(result, indent=2))
```

---

## 4. 系统感知（System State Awareness）

### 4.1 全量状态总线（System State Bus）

**目标**：让脚本能够感知和操作整个系统状态，而不仅仅是文件系统。

```
┌─────────────────────────────────────────────────────────────┐
│                  System State Bus                            │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  State Providers (状态提供者)                               │
│  ┌────────────────────────────────────────────────────┐    │
│  │ • FileSystemProvider: 文件系统状态                  │    │
│  │ • BrowserProvider: 浏览器选中文本、当前 URL         │    │
│  │ • NotionProvider: 最后编辑的页面、未保存的内容     │    │
│  │ • SlackProvider: 未读消息、当前频道                 │    │
│  │ • CalendarProvider: 今日日程、下一个会议            │    │
│  │ • ClipboardProvider: 剪贴板内容                     │    │
│  └────────────────────────────────────────────────────┘    │
│                           ↓                                  │
│  State Query API                                             │
│  ┌────────────────────────────────────────────────────┐    │
│  │ • get_browser_selected_text()                       │    │
│  │ • get_notion_last_edited_page()                     │    │
│  │ • get_slack_unread_messages()                       │    │
│  │ • get_calendar_next_event()                         │    │
│  └────────────────────────────────────────────────────┘    │
│                           ↓                                  │
│  Action API (当没有 API 时，使用模拟交互)                   │
│  ┌────────────────────────────────────────────────────┐    │
│  │ • snapshot_capture(): 截图 + AX tree + OCR         │    │
│  │ • locate_element(text): 定位 UI 元素坐标           │    │
│  │ • simulate_click(x, y): 模拟鼠标点击               │    │
│  │ • simulate_type(text): 模拟键盘输入                │    │
│  └────────────────────────────────────────────────────┘    │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 4.2 示例：Notion 集成

**场景**：用户说"把这个财务报告发送到 Notion"

**执行流程**：
1. LLM 激活"个人财务审计" Skill
2. 生成审计报告（Excel）
3. 检测到需要发送到 Notion
4. 查询 System State Bus: `get_notion_last_edited_page()`
5. 如果有 Notion API：调用 API 上传
6. 如果没有 API：
   - `snapshot_capture()` 截取 Notion 界面
   - `locate_element("Upload")` 定位上传按钮
   - `simulate_click(x, y)` 点击上传
   - `simulate_type(file_path)` 输入文件路径

---

## 5. 实施计划

### Phase 1: 核心架构（2 周）

**目标**：实现协作式进化的基础设施

**任务**：
1. 实现 `SuccessManifest` 数据结构
2. 实现 `ConstraintValidator`
3. 实现 `SolidificationPipeline`
4. 集成到现有的 `skill_evolution` 模块

**交付物**：
- `core/src/skill_evolution/success_manifest.rs`
- `core/src/skill_evolution/constraint_validator.rs`
- `core/src/skill_evolution/solidification_pipeline.rs`

### Phase 2: 原型 Skill（1 周）

**目标**：实现"个人财务审计" Skill 原型

**任务**：
1. 创建 Skill 目录结构
2. 编写 SUCCESS_MANIFEST.md
3. 实现 PDF/CSV 解析工具
4. 实现 Excel 生成工具
5. 编写集成测试

**交付物**：
- `~/.aleph/skills/personal_finance_audit/`
- 完整的工具链和测试

### Phase 3: UI 集成（1 周）

**目标**：实现用户审核界面

**任务**：
1. 在 Control Plane UI 中添加 Skill 审核页面
2. 展示语义化的软约束
3. 实现批准/拒绝/修改流程
4. 集成到 Gateway RPC

**交付物**：
- Control Plane UI 更新
- Gateway RPC 方法: `skill.propose`, `skill.approve`, `skill.reject`

### Phase 4: System State Bus（2 周）

**目标**：实现系统感知能力

**任务**：
1. 设计 State Provider 接口
2. 实现 BrowserProvider（基于 Chrome DevTools Protocol）
3. 实现 ClipboardProvider
4. 实现 snapshot_capture + simulate_click

**交付物**：
- `core/src/system_state/`
- 浏览器、剪贴板集成

---

## 6. 安全考虑

### 6.1 双层约束验证

**软约束（LLM 自觉）**：
- LLM 理解 SUCCESS_MANIFEST.md
- 主动遵守约束，提高效率

**硬约束（沙箱强制）**：
- macOS sandbox-exec 强制执行
- 即使 LLM 犯错或被攻击，也能阻止

### 6.2 约束不一致检测

```rust
// 当 LLM 尝试执行工具时
if soft_constraint_violated {
    log_warning("LLM attempted to violate semantic constraint");
    // 继续执行，让沙箱来阻止
}

if sandbox_blocked {
    log_error("Sandbox blocked execution");
    // 检查是否也违反了软约束
    if !soft_constraint_violated {
        log_critical("Sandbox blocked but soft constraint allowed - constraint mismatch!");
        // 这表明约束配置不一致，需要人工审核
    }
}
```

### 6.3 审计日志

所有 Skill 执行都会记录：
- 激活的 Skill
- 执行的工具
- 软约束检查结果
- 硬约束执行结果
- 任何约束违规尝试

---

## 7. 未来展望

### 7.1 Skill 市场

- 用户可以分享自己的 Skill
- 社区审核和评分
- 自动安全扫描

### 7.2 跨平台支持

- Linux: seccomp-bpf + AppArmor
- Windows: Job Objects + AppContainer

### 7.3 AI 辅助优化

- 自动检测 Skill 的性能瓶颈
- 建议更高效的工具组合
- 自动生成测试用例

---

## 8. 总结

**核心创新**：
1. **协作式进化**：用户作为"合规官"，LLM 作为"执行者"
2. **约束自动推导**：LLM 同时生成软约束和硬约束，系统自动校验
3. **双层约束**：软约束（语义）+ 硬约束（沙箱）
4. **系统感知**：不仅是文件，还能感知和操作整个系统状态

**价值主张**：
- 80% 的软件都会消失
- 用户只需描述意图，不需要学习软件
- LLM 在约束下安全地执行任务
- 持续进化，越用越智能
