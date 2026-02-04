# Aleph 超越 OpenClaw 设计文档

> 设计日期: 2026-01-31
> 状态: 已验证，待实现

---

## 1. 核心定位

**Aleph 的角色**：不是 OpenClaw 的简单克隆，而是用 Rust 重写并超越的系统。

| 维度 | OpenClaw | Aleph |
|------|----------|--------|
| 语言 | TypeScript (2000+ 文件) | Rust (255K+ 行，54 模块) |
| Memory | Vector + FTS5 | **Ebbinghaus 衰减 + 关联聚类 + 三路冲突** |
| 自我进化 | 用户手动引导 | **原生混合模式**（Memory → Skill 自动建议） |
| Claude Code | 无原生支持 | **PtySupervisor 监管者模式** |
| 安全模型 | 基础分级 | **规则引擎 + 确定性判断**（不依赖 LLM） |
| 韧性执行 | 基础重试 | **三级防御**（重试 → 降级 → 通知） |

**核心隐喻**：
- Aleph 是 **"AI Manager"**，指挥其他 AI（如 Claude Code）干活
- Aleph 是 **"产品经理 + 架构师"**，Claude Code 是 **"高级工程师"**

---

## 2. 六大核心系统架构

```
┌─────────────────────────────────────────────────────────────────────┐
│                         AETHER CORE                                  │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐          │
│  │   Memory     │    │    Skill     │    │  Security    │          │
│  │   System     │◄──►│   Evolution  │    │   Kernel     │          │
│  │ (Ebbinghaus) │    │   (Hybrid)   │    │ (Rule Engine)│          │
│  └──────────────┘    └──────────────┘    └──────────────┘          │
│         │                   │                   │                   │
│         └───────────────────┼───────────────────┘                   │
│                             │                                        │
│                    ┌────────▼────────┐                              │
│                    │  PtySupervisor  │                              │
│                    │ (Claude Code    │                              │
│                    │  Controller)    │                              │
│                    └────────┬────────┘                              │
│                             │                                        │
│         ┌───────────────────┼───────────────────┐                   │
│         │                   │                   │                   │
│  ┌──────▼──────┐    ┌──────▼──────┐    ┌──────▼──────┐            │
│  │  Telegram   │    │   Cron      │    │  Gateway    │            │
│  │  (Primary)  │    │ (Resilient) │    │ (WebSocket) │            │
│  └─────────────┘    └─────────────┘    └─────────────┘            │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

| 系统 | 职责 | 关键技术 |
|------|------|----------|
| **Memory System** | 短期经验积累，长期记忆衰减 | sqlite-vec, Ebbinghaus decay |
| **Skill Evolution** | 经验验证后建议固化到 Skill | 混合模式，Git 自动提交 |
| **Security Kernel** | 命令风险评估，确定性判断 | Regex 规则引擎，四级分类 |
| **PtySupervisor** | 操控 Claude Code，拦截审批 | portable-pty, ANSI parser |
| **Telegram Adapter** | 主力推送渠道，远程审批 | teloxide, inline keyboard |
| **Cron Scheduler** | 定时任务，韧性执行 | 三级防御，智能降级 |

---

## 3. PtySupervisor 详细设计

**核心理念**：Aleph 不重复造轮子，而是成为 Claude Code 的"监管者"。

### 3.1 架构图

```
┌─────────────────────────────────────────────────────────────┐
│                    PtySupervisor                             │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌─────────────┐         ┌─────────────┐                    │
│  │ Spec Writer │         │ Test Writer │                    │
│  │  (LLM生成   │         │  (LLM生成   │                    │
│  │   规格文档)  │         │   测试用例)  │                    │
│  └──────┬──────┘         └──────┬──────┘                    │
│         │                       │                            │
│         └───────────┬───────────┘                            │
│                     ▼                                        │
│  ┌─────────────────────────────────────┐                    │
│  │           PTY Master                 │                    │
│  │  • portable-pty 虚拟终端             │                    │
│  │  • stdin: 发送指令                   │                    │
│  │  • stdout: 读取思考过程              │                    │
│  └──────────────────┬──────────────────┘                    │
│                     │                                        │
│  ┌──────────────────▼──────────────────┐                    │
│  │         ANSI Parser Layer           │                    │
│  │  • strip_ansi_escapes 清洗          │                    │
│  │  • 语义触发 (Regex 匹配)             │                    │
│  │  • SecretMasker 脱敏                │                    │
│  └──────────────────┬──────────────────┘                    │
│                     │                                        │
│  ┌──────────────────▼──────────────────┐                    │
│  │          Event Triggers             │                    │
│  │  • "Do you want to run?" → 审批流   │                    │
│  │  • "Context window full" → /compact │                    │
│  │  • "Error:" → 重试或报告            │                    │
│  └──────────────────┬──────────────────┘                    │
│                     ▼                                        │
│  ┌─────────────────────────────────────┐                    │
│  │           LLM Judge                  │                    │
│  │  • 运行测试，判断成功/失败           │                    │
│  │  • 失败 → 注入修复指令               │                    │
│  │  • 成功 → Git Commit + Memory 存储   │                    │
│  └─────────────────────────────────────┘                    │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 3.2 Rust 实现

```rust
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::io::{Read, Write};

pub struct ClaudeSupervisor {
    writer: Box<dyn Write + Send>,
    reader: Box<dyn Read + Send>,
    master: Box<dyn portable_pty::MasterPty>,
}

impl ClaudeSupervisor {
    pub fn spawn(workspace_path: &str) -> Self {
        let pty_system = NativePtySystem::default();

        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        }).unwrap();

        let mut cmd = CommandBuilder::new("claude");
        cmd.cwd(workspace_path);

        let _child = pair.slave.spawn_command(cmd).unwrap();

        Self {
            writer: pair.master.take_writer().unwrap(),
            reader: pair.master.take_reader().unwrap(),
            master: pair.master,
        }
    }
}

// ANSI 清洗与语义解析
fn monitor_output(mut reader: Box<dyn Read + Send>, event_bus: EventBus) {
    let mut buffer = [0u8; 1024];

    loop {
        let n = reader.read(&mut buffer).unwrap();
        let raw_text = strip_ansi_escapes(&buffer[..n]);

        if raw_text.contains("Do you want to run this command?") {
            event_bus.emit(SystemEvent::ApprovalRequest(raw_text));
        } else if raw_text.contains("Context window is full") {
            event_bus.emit(SystemEvent::ContextOverflow);
        }

        event_bus.emit(RunEvent::Log(raw_text));
    }
}
```

### 3.3 规格驱动开发闭环

1. 用户输入需求 → Aleph 生成 `specs/*.spiky` + `tests/*_test.py`
2. PtySupervisor 启动 Claude Code → 注入指令
3. 拦截确认提示 → SecurityKernel 判断 → 自动/手动审批
4. 任务完成 → LLM Judge 运行测试
5. 失败 → 注入修复指令（循环）
6. 成功 → 经验存入 Memory，Git Commit

---

## 4. SecurityKernel 详细设计

**核心理念**：安全必须是确定性的，不依赖 LLM 的概率判断。

### 4.1 四级红绿灯协议

```
┌─────────────────────────────────────────────────────────────┐
│                    SecurityKernel                            │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  命令输入: "rm -rf ./temp_project"                          │
│                     │                                        │
│                     ▼                                        │
│  ┌─────────────────────────────────────┐                    │
│  │  Level 1: Blocked Patterns          │                    │
│  │  • rm -rf /                         │ → ⛔ 绝对禁止      │
│  │  • :(){ :|:& };: (Fork Bomb)        │    立即拒绝        │
│  │  • dd if=/dev/zero of=/dev/sda      │                    │
│  └──────────────────┬──────────────────┘                    │
│                     │ 未匹配                                 │
│                     ▼                                        │
│  ┌─────────────────────────────────────┐                    │
│  │  Level 2: Danger Patterns           │                    │
│  │  • rm, sudo, mv (覆盖), chmod       │ → 🔴 Telegram 审批 │
│  │  • kill, shutdown, mkfs             │                    │
│  └──────────────────┬──────────────────┘                    │
│                     │ 未匹配                                 │
│                     ▼                                        │
│  ┌─────────────────────────────────────┐                    │
│  │  Level 3: Safe Patterns             │                    │
│  │  • ls, cat, grep, git status        │ → 🟢 静默放行      │
│  │  • pwd, echo, which                 │                    │
│  └──────────────────┬──────────────────┘                    │
│                     │ 未匹配                                 │
│                     ▼                                        │
│  ┌─────────────────────────────────────┐                    │
│  │  Default: Caution                   │ → 🟡 放行+记录     │
│  │  • npm install, cargo build         │    UI 黄色角标     │
│  │  • docker run, curl                 │                    │
│  └─────────────────────────────────────┘                    │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 4.2 Rust 实现

```rust
#[derive(Debug, PartialEq, PartialOrd)]
pub enum RiskLevel {
    Safe,       // 绿灯
    Caution,    // 黄灯
    Danger,     // 红灯
    Blocked,    // 黑名单
}

pub struct CommandPolicy {
    safe_patterns: Vec<Regex>,
    danger_patterns: Vec<Regex>,
    blocked_patterns: Vec<Regex>,
}

impl SecurityKernel {
    pub fn assess(&self, cmd: &str) -> RiskLevel {
        // 1. 绝对防御
        for pattern in &self.blocked_patterns {
            if pattern.is_match(cmd) {
                return RiskLevel::Blocked;
            }
        }

        // 2. 危险检测
        for pattern in &self.danger_patterns {
            if pattern.is_match(cmd) {
                return RiskLevel::Danger;
            }
        }

        // 3. 安全检测
        for pattern in &self.safe_patterns {
            if pattern.is_match(cmd) {
                return RiskLevel::Safe;
            }
        }

        RiskLevel::Caution
    }
}
```

### 4.3 Secret Masking

```rust
fn redact_secrets(raw_output: String) -> String {
    let mut clean = raw_output;

    // OpenAI Key
    let sk_regex = Regex::new(r"sk-[a-zA-Z0-9]{48}").unwrap();
    clean = sk_regex.replace_all(&clean, "sk-***REDACTED***").to_string();

    // Anthropic Key
    let anthropic_regex = Regex::new(r"sk-ant-[a-zA-Z0-9\-]{90,}").unwrap();
    clean = anthropic_regex.replace_all(&clean, "sk-ant-***REDACTED***").to_string();

    clean
}
```

---

## 5. ResilientTask 详细设计

**核心理念**：用户想看到的是**内容**，不是**报错**。

### 5.1 三级防御体系

```
┌─────────────────────────────────────────────────────────────┐
│                    ResilientTask                             │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  Level 1: 静默重试                                          │
│  • 最多 3 次，指数退避 (2s → 4s → 8s)                       │
│  • 场景: API 超时、网络抖动                                  │
│                     │                                        │
│                     ▼ 全部失败                               │
│  Level 2: 智能降级                                          │
│  • TTS 失败 → 生成 Markdown 摘要                            │
│  • 通知: "⚠️ 已切换备用方案"                                │
│                     │                                        │
│                     ▼ 降级也失败                             │
│  Level 3: 最终通知                                          │
│  • Telegram 发送错误报告                                     │
│  • 附带 [立即重试] 按钮                                      │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 5.2 Rust 实现

```rust
use async_trait::async_trait;

pub enum TaskResult {
    Success(String),
    Degraded(String, String),  // (内容, 降级原因)
    Failed(String),
}

#[async_trait]
pub trait ResilientTask: Send + Sync {
    fn name(&self) -> &str;

    async fn execute(&self) -> anyhow::Result<String>;

    async fn fallback(&self, _error: &anyhow::Error) -> anyhow::Result<String> {
        Err(anyhow::anyhow!("No fallback available"))
    }

    fn retry_count(&self) -> usize { 3 }
}

pub async fn run_task(task: Box<dyn ResilientTask>, notifier: &TelegramAdapter) {
    let mut attempt = 0;

    loop {
        attempt += 1;
        match task.execute().await {
            Ok(output) => {
                notifier.send(&output, NotificationLevel::Info).await.ok();
                return;
            }
            Err(e) => {
                if attempt < task.retry_count() {
                    sleep(Duration::from_secs(2u64.pow(attempt as u32))).await;
                    continue;
                }

                match task.fallback(&e).await {
                    Ok(fallback_output) => {
                        let msg = format!(
                            "⚠️ {}: 主服务异常，已切换至备用方案。\n\n{}",
                            task.name(),
                            fallback_output
                        );
                        notifier.send(&msg, NotificationLevel::Warning).await.ok();
                        return;
                    }
                    Err(fatal_error) => {
                        let msg = format!("❌ {}: 执行失败。\n原因: {}", task.name(), fatal_error);
                        notifier.send(&msg, NotificationLevel::ActionRequired {
                            options: vec!["Retry Now".to_string()]
                        }).await.ok();
                        return;
                    }
                }
            }
        }
    }
}
```

---

## 6. Skill 进化系统详细设计

**核心理念**：Memory 负责短期积累，Skill 负责长期固化，AI 主动建议而非自动修改。

### 6.1 四阶段流程

```
Stage 1: Memory 短期存储
  • 经验写入 Memory (Ebbinghaus 衰减)
  • 关联聚类：相似经验自动归组

Stage 2: 经验验证追踪
  • 相同问题出现 2+ 次？
  • 相同解决方案成功 2+ 次？
  • 经验被检索并采纳 3+ 次？

Stage 3: 固化建议 (触发阈值后)
  💡 "我注意到您多次遇到 Vite HMR 端口冲突问题，
      每次都通过修改 vite.config.ts 解决。
      是否要将此经验固化到 Skill？"
  [✅ 固化到 Skill] [⏸️ 稍后再说]

Stage 4: Skill 自动更新 (用户确认后)
  • 生成 Skill 补丁
  • 用户预览 diff
  • 写入 ~/.aleph/skills/
  • Git Commit + Push
```

### 6.2 Memory ↔ Skill 联动规则

| 阶段 | 存储位置 | 生命周期 | 触发条件 |
|------|----------|----------|----------|
| 新经验 | Memory (Layer 2 Facts) | 受 Ebbinghaus 衰减影响 | 任务执行后自动 |
| 验证中 | Memory (标记为 `validated`) | 衰减减缓 | 成功复用 2+ 次 |
| 已固化 | Skill 文件 | 永久 | 用户确认固化 |

**关键约束**：AI **永远不会**未经用户确认就修改 Skill 文件。

---

## 7. 实现路线图

### Milestone 依赖关系

```
M1 (PtySupervisor) ──┬──► M4 (规格驱动)
                     │
M2 (SecurityKernel) ─┼──► M3 (Telegram 审批)
                     │
M5 (Skill 进化) ─────┘

M6 (韧性执行) ──────────► 独立，可并行开发
```

### Milestone 1: PtySupervisor 基础

- [x] 集成 portable-pty crate
- [x] 实现 ClaudeSupervisor::spawn()
- [x] ANSI 清洗层 (strip_ansi_escapes)
- [x] 基础 stdin/stdout 交互测试

**验收**: ✅ Aleph 能启动 Claude Code 并读取输出

### Milestone 2: SecurityKernel 规则引擎

- [x] 定义 RiskLevel 四级枚举
- [x] 实现 CommandPolicy (Regex 规则集)
- [x] SecurityKernel::assess() 零延迟判断
- [x] SecretMasker 敏感信息脱敏

**验收**: ✅ rm -rf / 被 Blocked，ls 被 Safe

### Milestone 3: Telegram 审批集成

- [x] TelegramAdapter 增强 (inline keyboard)
- [x] 审批请求消息模板
- [x] 回调处理 (approve/reject)
- [x] PtySupervisor ↔ Telegram 联动

**验收**: ✅ Danger 命令触发 Telegram 弹窗，点击后放行

### Milestone 4: 规格驱动开发闭环

- [x] SpecWriter (LLM 生成规格)
- [x] TestWriter (LLM 生成测试用例)
- [x] LlmJudge (评估实现，判断成功/失败)
- [x] SpecDrivenWorkflow (迭代重试循环)

**验收**: ✅ 输入需求 → 全自动生成可运行代码

### Milestone 5: Skill 进化系统

- [x] Memory 经验验证追踪 (EvolutionTracker - use_count, success_count)
- [x] 固化建议生成器 (SolidificationDetector - 阈值触发)
- [x] Skill 补丁生成 + diff 预览 (SkillGenerator)
- [x] Git 自动 Commit + Push (GitCommitter)

**验收**: ✅ 重复解决同一问题 3 次后，AI 主动建议固化

### Milestone 6: ResilientTask 韧性执行

- [x] ResilientTask trait 定义 (execute/fallback/has_fallback/config)
- [x] 重试策略 (指数退避 + Jitter)
- [x] 降级逻辑 (Skip/Fallback/PartialResult/UseCached/NotifyAndFail)
- [x] 与 Cron 系统集成 (ResilientCronJob + PodcastTask)

**验收**: ✅ 播客 TTS 失败时自动降级为 Markdown 摘要

---

## 8. 技术栈总结

| 组件 | 技术选型 | 理由 |
|------|----------|------|
| PTY 控制 | portable-pty | 跨平台虚拟终端，欺骗 TUI 程序 |
| 安全规则 | regex crate | 零延迟，确定性判断 |
| 消息推送 | teloxide | Rust 原生 Telegram Bot 库 |
| 内存系统 | sqlite-vec + fastembed | 向量搜索 + 本地嵌入 |
| 异步运行时 | tokio | 业界标准 |
| 序列化 | serde + serde_json | 业界标准 |

---

## 9. 设计决策记录

| 决策点 | 选项 | 最终选择 | 理由 |
|--------|------|----------|------|
| 自我进化方式 | 原生内置 / 增强 Memory / Skill 热演化 / 混合模式 | **混合模式** | 平衡自动化与用户控制 |
| Claude Code 集成 | 轻量 / 进程控制 / MCP / 深度融合 | **进程控制 + 监管者模式** | 最稳健解耦，复用 Claude Code 能力 |
| 多渠道策略 | Telegram / Discord / iMessage / 全渠道 | **Telegram 优先，全渠道架构** | 战术聚焦，战略扩展 |
| 安全模型 | 严格 / AI Judge / 分级 / 信任升级 | **分级模式 + 规则引擎** | 确定性判断，不依赖 LLM |
| 失败处理 | 静默重试 / 立即通知 / 智能降级 / 队列延后 | **智能降级 + 静默重试** | 用户要内容，不要报错 |

---

*文档生成时间: 2026-01-31*
*状态: 六大里程碑全部完成 ✅*
