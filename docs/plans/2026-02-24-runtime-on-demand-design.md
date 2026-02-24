# Runtime On-Demand: Native Bootstrapping Architecture

> **方案 D: 原生引导架构 (Native Bootstrapping Architecture)**
>
> 从"带工具箱的软件"变为"会自己造工具的生命体"

**Date**: 2026-02-24
**Status**: Approved
**Scope**: core/src/runtimes/ 重构，init 流程改造，exec/prompt 集成

---

## 1. 背景与动机

### 现状分析

当前 `core/src/runtimes/` 包含 ~2500 行 Rust 代码，主要职责是通过 `reqwest` 下载二进制文件并解压安装 4 个运行时 (uv, fnm, ffmpeg, yt-dlp)。

**关键问题**：

1. **强制初始化**：`init_unified/coordinator.rs` 在启动时并行安装全部 4 个运行时，阻塞用户数秒
2. **Registry 碎片化**：消费者各自创建 `RuntimeRegistry::new()`，无全局共享
3. **exec 断裂**：`build_aleph_path()` 已实现但从未接入 exec 层
4. **Capability 死代码**：`format_for_prompt()` 已实现但从未接入 prompt 系统
5. **违反 R3 红线**：Core 亲自"搬砖"下载二进制，而非调度四肢

### 设计哲学

> 将运行时管理权从 Skeleton（骨架/硬编码逻辑）移交给 Soul（灵魂/Agent 逻辑）。

- **R3 核心轻量化**：Core 只调度，不搬砖
- **Native-Powered**：与宿主环境深度集成，优先使用系统已有工具
- **Frictionless**：零启动负载，运行时在对话中按需觉醒
- **L5 随需变身**：Aleph 通过 Shell 自己获取所需能力

---

## 2. 架构概览

### 核心组件

```
┌──────────────────────────────────────────────────────────────────┐
│                        Agent Loop                                │
│                                                                  │
│  Observe ──→ Think ──→ Act ──→ Feedback ──→ Compress            │
│                │         │                                       │
│                │         ▼                                       │
│                │    Dispatcher                                   │
│                │         │                                       │
│                │    ensure_capability()                           │
│                │         │                                       │
│                │    ┌────┴────┐                                   │
│                │    ▼         ▼                                   │
│                │  Ledger   Probe ──→ Bootstrap ──→ Register      │
│                │  (查询)   (探测)     (Shell安装)    (更新Ledger)   │
│                │    │                                            │
│                ▼    ▼                                            │
│         PromptBuilder                                           │
│         (注入 runtime_capabilities)                              │
│                                                                  │
│  Exec Layer ←── enhanced PATH from Ledger.build_path()          │
└──────────────────────────────────────────────────────────────────┘
```

### 三步引导协议

```
┌─────────┐     ┌─────────────┐     ┌──────────┐
│  Probe  │ ──→ │  Bootstrap  │ ──→ │ Register │
│ (探测)   │     │ (引导安装)    │     │ (登记)    │
└─────────┘     └─────────────┘     └──────────┘
  系统优先          Shell 驱动          更新 Ledger
```

---

## 3. 删减清单

| 文件 | 当前 LOC | 处置 | 理由 |
|------|----------|------|------|
| `uv.rs` | ~375 | **删除** | 安装逻辑迁移到 Shell |
| `fnm.rs` | ~513 | **删除** | 安装逻辑迁移到 Shell |
| `ffmpeg.rs` | ~310 | **删除** | 安装逻辑迁移到 Shell |
| `ytdlp.rs` | ~178 | **删除** | 安装逻辑迁移到 Shell |
| `download.rs` | ~400 | **删除** | reqwest 下载逻辑不再需要 |
| `manager.rs` | - | **重构** | RuntimeManager trait 脱水为极简接口 |
| `registry.rs` | ~312 | **重构** | 瘦身为 CapabilityLedger |
| `manifest.rs` | ~259 | **保留** | 迁移期间需要读取旧格式 |
| `capability.rs` | - | **保留+接通** | 补全与 prompt 系统的断线 |
| `git_check.rs` | ~117 | **合并** | 纳入统一的探测逻辑 |
| `mod.rs` | - | **重构** | build_aleph_path 接入 exec 层 |

**净效果**：删除 ~1776 行 Rust 代码，保留 ~700 行核心骨架。

---

## 4. Capability Ledger（能力账本）

### 数据结构

```rust
// core/src/runtimes/ledger.rs

/// 能力状态机
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum CapabilityStatus {
    Missing,        // 未知，从未探测
    Probing,        // 正在探测系统
    Bootstrapping,  // 正在通过 Shell 安装
    Ready,          // 可用，路径已验证
    Stale,          // 曾可用但路径失效
}

/// 能力来源
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CapabilitySource {
    System,         // 来自系统 PATH
    AlephManaged,   // 由 Aleph 安装到托管目录
}

/// 单条能力记录
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapabilityEntry {
    pub name: String,
    pub bin_path: Option<PathBuf>,
    pub version: Option<String>,
    pub status: CapabilityStatus,
    pub source: CapabilitySource,
    pub last_probed: Option<SystemTime>,
}

/// 能力账本 — 全局唯一，Arc<RwLock<>> 共享
pub struct CapabilityLedger {
    entries: HashMap<String, CapabilityEntry>,
    persist_path: PathBuf,  // ~/.aleph/runtimes/ledger.json
}
```

### 职责边界

| 做 | 不做 |
|----|------|
| 记录能力名称、路径、版本、状态 | 下载二进制文件 |
| 持久化到 JSON | 解压/校验安装包 |
| 提供快速查询接口 | 执行安装命令 |
| 状态机转换 | 知道"如何安装"特定运行时 |

### 关键方法

```rust
impl CapabilityLedger {
    pub fn load_or_create(path: PathBuf) -> Result<Self>;
    pub fn status(&self, name: &str) -> CapabilityStatus;
    pub fn executable(&self, name: &str) -> Option<&Path>;
    pub fn update(&mut self, entry: CapabilityEntry) -> Result<()>;
    pub fn build_path(&self) -> String;
    pub fn list_ready(&self) -> Vec<&CapabilityEntry>;
    pub fn persist(&self) -> Result<()>;
}
```

---

## 5. Probe（系统探测）

### System-First 策略

```rust
// core/src/runtimes/probe.rs

pub struct ProbeResult {
    pub found: bool,
    pub bin_path: Option<PathBuf>,
    pub version: Option<String>,
    pub source: CapabilitySource,
}

/// 探测链：Aleph 托管 → 系统 PATH → 已知路径
pub async fn probe(name: &str) -> ProbeResult {
    if let Some(r) = probe_aleph_managed(name).await { return r; }
    if let Some(r) = probe_system_path(name).await { return r; }
    if let Some(r) = probe_known_paths(name).await { return r; }
    ProbeResult { found: false, .. }
}
```

### 探测配置表

```rust
struct ProbeSpec {
    capability: &'static str,
    binaries: &'static [&'static str],
    version_cmd: &'static str,
    version_regex: &'static str,
    min_version: Option<&'static str>,  // 降级使用 + 警告
}

const PROBE_SPECS: &[ProbeSpec] = &[
    ProbeSpec { capability: "python", binaries: &["python3", "python"],
                version_cmd: "--version", version_regex: r"Python (\d+\.\d+\.\d+)",
                min_version: Some("3.10") },
    ProbeSpec { capability: "node", binaries: &["node"],
                version_cmd: "--version", version_regex: r"v(\d+\.\d+\.\d+)",
                min_version: Some("18.0") },
    ProbeSpec { capability: "ffmpeg", binaries: &["ffmpeg"],
                version_cmd: "-version", version_regex: r"ffmpeg version (\S+)",
                min_version: None },
    ProbeSpec { capability: "uv", binaries: &["uv"],
                version_cmd: "--version", version_regex: r"uv (\d+\.\d+\.\d+)",
                min_version: None },
    ProbeSpec { capability: "yt-dlp", binaries: &["yt-dlp"],
                version_cmd: "--version", version_regex: r"(\d{4}\.\d+\.\d+)",
                min_version: None },
];
```

### 版本不兼容策略

当系统工具版本低于 `min_version` 时：**降级使用 + 在 prompt 中注入版本警告**，让 AI 自行判断是否需要提示用户升级。

---

## 6. Bootstrap（引导安装）

### Shell 驱动安装

安装逻辑以内嵌脚本形式存在于 Rust 二进制中（`&'static str`），非外部文件。

```rust
// core/src/runtimes/bootstrap.rs

struct BootstrapSpec {
    capability: &'static str,
    script_macos: &'static str,
    script_linux: &'static str,
    expected_path: &'static str,
}

const BOOTSTRAP_SPECS: &[BootstrapSpec] = &[
    BootstrapSpec {
        capability: "uv",
        script_macos: "curl -LsSf https://astral.sh/uv/install.sh | sh",
        script_linux: "curl -LsSf https://astral.sh/uv/install.sh | sh",
        expected_path: "~/.local/bin/uv",
    },
    BootstrapSpec {
        capability: "python",
        script_macos: "uv python install 3.12 && uv venv ~/.aleph/runtimes/python/default",
        script_linux: "uv python install 3.12 && uv venv ~/.aleph/runtimes/python/default",
        expected_path: "~/.aleph/runtimes/python/default/bin/python3",
    },
    BootstrapSpec {
        capability: "node",
        script_macos: "curl -fsSL https://fnm.vercel.app/install | bash && fnm install --lts",
        script_linux: "curl -fsSL https://fnm.vercel.app/install | bash && fnm install --lts",
        expected_path: "~/.local/share/fnm/aliases/default/bin/node",
    },
    BootstrapSpec {
        capability: "ffmpeg",
        script_macos: "brew install ffmpeg",
        script_linux: "sudo apt-get install -y ffmpeg",
        expected_path: "/usr/local/bin/ffmpeg",
    },
    BootstrapSpec {
        capability: "yt-dlp",
        script_macos: "uv tool install yt-dlp",
        script_linux: "uv tool install yt-dlp",
        expected_path: "~/.local/bin/yt-dlp",
    },
];
```

### 依赖链

```rust
fn dependencies(capability: &str) -> &[&str] {
    match capability {
        "python" => &["uv"],
        "yt-dlp" => &["uv"],
        _ => &[],
    }
}
```

### 执行引导

```rust
pub async fn bootstrap(
    capability: &str,
    exec_context: &ExecContext,
) -> Result<BootstrapResult> {
    let spec = find_spec(capability)?;
    let script = platform_script(spec);

    // 通过 exec 安全层执行
    let output = exec_context.run_shell(script).await?;

    if output.success {
        let path = expand_path(spec.expected_path);
        if path.exists() {
            Ok(BootstrapResult::Success { bin_path: path })
        } else {
            Ok(BootstrapResult::PathNotFound { expected: path })
        }
    } else {
        Ok(BootstrapResult::Failed { stderr: output.stderr })
    }
}
```

---

## 7. 系统集成

### A. Exec 层 — PATH 注入

```rust
// core/src/builtin_tools/code_exec.rs

impl CodeExecTool {
    pub async fn execute(&self, args: CodeExecArgs, ledger: &CapabilityLedger) -> Result<Output> {
        let mut cmd = Command::new(shell);
        cmd.env_clear();

        // 使用 Ledger 构建的增强 PATH
        let enhanced_path = ledger.build_path();
        cmd.env("PATH", enhanced_path);

        // 其余 env 变量照旧
        for var in &self.pass_env {
            if var != "PATH" {
                if let Ok(value) = std::env::var(var) {
                    cmd.env(var, value);
                }
            }
        }
    }
}
```

### B. Prompt 系统 — 能力注入

接通现有死代码管道：

```rust
// Agent Loop Think 阶段
let ready_capabilities = ledger.list_ready();
let capability_text = RuntimeCapability::format_for_prompt(&ready_capabilities);
prompt_config.runtime_capabilities = Some(capability_text);
```

### C. Dispatcher — 引导调度

```rust
pub async fn ensure_capability(
    capability: &str,
    ledger: &Arc<RwLock<CapabilityLedger>>,
    exec_ctx: &ExecContext,
    feedback_tx: &Sender<UserFeedback>,
) -> Result<PathBuf> {
    match ledger.read().await.status(capability) {
        CapabilityStatus::Ready => {
            Ok(ledger.read().await.executable(capability).unwrap().to_owned())
        }
        CapabilityStatus::Missing | CapabilityStatus::Stale => {
            // 1. Probe
            ledger.write().await.update_status(capability, CapabilityStatus::Probing)?;
            let probe_result = probe::probe(capability).await;

            if probe_result.found {
                ledger.write().await.update(CapabilityEntry::from_probe(capability, probe_result))?;
                return Ok(probe_result.bin_path.unwrap());
            }

            // 2. Resolve dependencies
            for dep in bootstrap::dependencies(capability) {
                Box::pin(ensure_capability(dep, ledger, exec_ctx, feedback_tx)).await?;
            }

            // 3. Bootstrap
            feedback_tx.send(UserFeedback::Status(
                format!("正在准备 {} 运行时...", capability)
            )).await?;

            ledger.write().await.update_status(capability, CapabilityStatus::Bootstrapping)?;
            match bootstrap::bootstrap(capability, exec_ctx).await? {
                BootstrapResult::Success { bin_path } => {
                    ledger.write().await.update(CapabilityEntry::new_ready(
                        capability, bin_path.clone(), CapabilitySource::AlephManaged
                    ))?;
                    Ok(bin_path)
                }
                BootstrapResult::Failed { stderr } => {
                    ledger.write().await.update_status(capability, CapabilityStatus::Missing)?;
                    Err(AlephError::runtime(format!(
                        "无法自动安装 {}。错误: {}。请手动安装后重试。",
                        capability, stderr
                    )))
                }
            }
        }
        CapabilityStatus::Bootstrapping | CapabilityStatus::Probing => {
            wait_for_ready(capability, ledger).await
        }
    }
}
```

### D. Init 阶段改动

```rust
// init_unified/coordinator.rs

// 之前（删除）：
// let runtime_ids = ["ffmpeg", "yt-dlp", "uv", "fnm"];
// for id in runtime_ids { registry.require(id).await?; }

// 之后：仅创建空 Ledger，零 IO
let ledger = CapabilityLedger::load_or_create(runtimes_dir.join("ledger.json"))?;
app_context.set_capability_ledger(Arc::new(RwLock::new(ledger)));
```

---

## 8. 风险控制

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|----------|
| **冷启动怪圈** | 低 | 高 | Bootstrap 只依赖 Shell (curl + sh)，不依赖高级运行时。依赖链: Shell → uv → Python |
| **网络不可用** | 中 | 中 | Probe 优先发现系统工具；失败时返回清晰错误，Agent 引导用户手动处理 |
| **脚本执行安全** | 低 | 高 | 脚本硬编码在 Rust 二进制中；通过 exec 安全层执行，遵循审批流程 |
| **并发引导竞态** | 中 | 低 | Ledger `Bootstrapping` 状态 + `wait_for_ready()` 信号量 |
| **系统工具版本过旧** | 中 | 低 | 降级使用 + prompt 注入版本警告，AI 自行判断 |
| **Ledger 损坏** | 低 | 低 | 损坏时重建空 Ledger，所有能力变 Missing，下次使用时重新探测 |

---

## 9. 迁移策略

```rust
pub fn migrate_from_legacy(runtimes_dir: &Path) -> Result<CapabilityLedger> {
    let legacy_manifest = runtimes_dir.join("manifest.json");
    let new_ledger_path = runtimes_dir.join("ledger.json");

    if legacy_manifest.exists() && !new_ledger_path.exists() {
        let manifest: LegacyManifest = serde_json::from_str(
            &fs::read_to_string(&legacy_manifest)?
        )?;

        let mut ledger = CapabilityLedger::new(new_ledger_path);
        for (id, metadata) in manifest.runtimes {
            ledger.update(CapabilityEntry {
                name: id,
                bin_path: None,
                version: Some(metadata.version),
                status: CapabilityStatus::Stale,  // 需重新 Probe
                source: CapabilitySource::AlephManaged,
                last_probed: None,
            })?;
        }

        ledger.persist()?;
        Ok(ledger)
    } else {
        CapabilityLedger::load_or_create(new_ledger_path)
    }
}
```

---

## 10. 实施阶段

| 阶段 | 目标 | 交付物 | 验证标准 |
|------|------|--------|----------|
| **Phase 1: 解耦** | 取消强制 init，Ledger 替代 Registry | ledger.rs + init 改动 | 启动时间从秒级降至毫秒级 |
| **Phase 2: 集成** | System-First 探测 + exec PATH 注入 + prompt 接入 | probe.rs + exec 改动 + capability 接通 | AI 能感知已有运行时；Shell 能找到 Aleph 管理的工具 |
| **Phase 3: 引导** | Shell Bootstrap + Dispatcher 调度 | bootstrap.rs + Dispatcher 改动 | 对话中首次使用 Python 时自动安装 |
| **Phase 4: 清理** | 删除旧代码 | 删除 ~1776 LOC | 编译通过 + 所有功能正常 |

---

## 11. 未来演进

1. **Runtime as a Skill**：将引导逻辑进一步抽象为 Agent 可发现的"自修复"技能
2. **智能环境共享**：识别并复用已有的 uv/fnm 缓存
3. **WASM 容器扩展**：对于危险的运行时需求，按需拉取轻量级沙箱
4. **声明式依赖**：Skill manifest 中声明 `requires: ["python>=3.10"]`，系统自动解析和满足
