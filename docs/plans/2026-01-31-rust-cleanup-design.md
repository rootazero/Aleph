# Aleph Rust 层代码清理规划

> 日期：2026-01-31
> 状态：待执行
> 范围：core/src/ 下 52 个模块的 FFI 残留清理

## 背景

Aleph 正在从 Library 模式（UniFFI）向 Service/Daemon 模式（Gateway WebSocket）转型。虽然 UniFFI 依赖和宏已被移除，但代码库中仍存在大量为 FFI 边界设计的类型和逻辑。

### 当前状态

| 指标 | 值 |
|------|-----|
| lib.rs 行数 | 592 |
| `pub use` 导出 | 44 |
| 编译警告数 | 22 |
| FFI 关键字文件 | 26 |
| `.clone()` 热点 | dispatcher/ (466), gateway/ (424) |

## 核心发现：FFI 残留分类

### Type A — 历史遗留型（可直接删除）

- **文件**：`intent/types/ffi.rs`（370 行）
- **特征**：定义了 `*FFI` 后缀的简化类型（如 `TaskCategoryFFI`），这些类型是因为 "UniFFI 不支持带关联数据的枚举" 而创建的
- **现状**：仅被 `lib.rs` 导出，无实际调用者
- **处置**：直接删除

### Type B — 转型复用型（需要重命名）

- **文件**：`dispatcher/types/ffi.rs`（255 行）
- **特征**：定义了 `ToolSourceType` 和 `UnifiedToolInfo`，虽然文件名带 `ffi`，但这些类型已经被 Gateway handlers 使用
- **现状**：11 个文件依赖，包括 `gateway/handlers/commands.rs`
- **处置**：保留内容，重命名文件（去掉 `ffi` 后缀）

## 清理策略

采用 **FFI 残留优先 + 编译警告驱动** 的组合策略：

```
Phase 0: 全局预检（编译警告地图）
Phase 1: 删除 Type A 残留（intent/types/ffi.rs）
Phase 2: 重构 Type B 残留（dispatcher/types/ffi.rs → types.rs）
Phase 3: 按模块扫荡（52 个目录逐个过）
Phase 4: 依赖瘦身（Cargo.toml unused deps）
```

## Phase 0: 全局预检

### Step 0.1: 创建安全锚点

```bash
git tag pre-cleanup-$(date +%Y%m%d)
git stash push -m "WIP before cleanup"
```

### Step 0.2: 启用严格警告模式

在 `lib.rs` 顶部临时添加：

```rust
#![warn(dead_code)]
#![warn(unused_imports)]
#![warn(unused_variables)]
```

### Step 0.3: 生成死代码地图

```bash
# 获取完整警告列表（作为清理 checklist）
RUSTFLAGS="-W dead_code" cargo check 2>&1 | tee cleanup-warnings.txt

# 统计各模块警告数
grep "warning:" cleanup-warnings.txt | \
  sed 's/.*src\/\([^/]*\)\/.*/\1/' | sort | uniq -c | sort -rn
```

### Step 0.4: 检查 Cargo.toml 残留依赖

```bash
cargo +nightly udeps 2>/dev/null || echo "Install: cargo install cargo-udeps"
```

## Phase 1: 删除 Type A 残留

**目标文件**：`src/intent/types/ffi.rs`

### Step 1.1: 确认无外部引用

```bash
grep -rn "TaskCategoryFFI\|ExecutableTaskFFI\|ParameterSourceFFI" src/ \
  --include="*.rs" | grep -v "intent/types/ffi.rs"
```

### Step 1.2: 从 lib.rs 移除导出

删除 lib.rs 中对这些类型的 `pub use`：

- `TaskCategoryFFI`
- `ExecutableTaskFFI`
- `ExecutionIntentTypeFFI`
- `AmbiguousTaskFFI`
- `ParameterSourceFFI`
- `OrganizeMethodFFI`
- `ConflictResolutionFFI`
- `TaskParametersFFI`

### Step 1.3: 删除文件并修复 mod.rs

```bash
rm src/intent/types/ffi.rs
# 编辑 src/intent/types/mod.rs，移除 `pub mod ffi;`
```

### Step 1.4: 验证编译

```bash
cargo check 2>&1 | grep -E "(error|warning)"
# 目标：只有 warning，没有 error
```

## Phase 2: Type B 身份洗白

**目标文件**：`src/dispatcher/types/ffi.rs` → 重命名并整合

### Step 2.1: 分析依赖图

```bash
grep -rn "types::ffi" src/ --include="*.rs"
```

预期依赖者（11 个文件）：

- `gateway/handlers/commands.rs`
- `dispatcher/registry/mod.rs`
- `command/parser.rs`
- `command/types.rs`
- `intent/decision/execution_decider.rs`

### Step 2.2: 重命名策略

**方案 A（推荐）：并入 types/mod.rs**

```bash
cat src/dispatcher/types/ffi.rs >> src/dispatcher/types/mod.rs
rm src/dispatcher/types/ffi.rs
```

**方案 B：重命名为语义化名称**

```bash
mv src/dispatcher/types/ffi.rs src/dispatcher/types/tool_info.rs
# 更新 mod.rs: pub mod ffi; → pub mod tool_info;
```

### Step 2.3: 全局替换 import 路径

```bash
# Before: use crate::dispatcher::types::ffi::{ToolSourceType, UnifiedToolInfo};
# After:  use crate::dispatcher::types::{ToolSourceType, UnifiedToolInfo};
```

### Step 2.4: 清理文档注释

移除文件头部关于 UniFFI 的过时注释：

```rust
// 删除这类注释：
// "UniFFI doesn't support enums with associated data"
// "Simplified types for Swift/Kotlin interop via UniFFI"
```

## Phase 3: 52 目录扫荡

### 优先级矩阵

#### Batch 1: 高优先级（FFI 重灾区）

| 目录 | .clone() 数 | 清理重点 |
|------|------------|---------|
| `dispatcher/` | 466 | 移除 FFI 类型转换层，统一用原生 Rust enum |
| `gateway/` | 424 | 检查是否有为 FFI 保留的兼容逻辑 |
| `memory/` | 152 | 移除同步包装器（Vec 克隆常见于 FFI 返回） |
| `generation/` | 120 | 移除流式 FFI 回调，改用 async Stream |

#### Batch 2: 中优先级（功能模块）

| 目录 | 清理重点 |
|------|---------|
| `extension/` | 移除旧 Plugin trait 适配 |
| `agents/` | 检查 sub-agent 的 FFI 桥接 |
| `intent/` | Phase 1 已删除 `types/ffi.rs`，检查残留引用 |
| `config/` | 移除 Swift ConfigManager 适配代码 |

#### Batch 3: 低优先级（新模块 / 纯 Rust）

| 目录 | 状态 |
|------|------|
| `wizard/` | 新模块，无 FFI 历史 |
| `supervisor/` | 新模块，无 FFI 历史 |
| `exec/` | 新模块，无 FFI 历史 |
| `routing/` | 新的 Session Key 系统，干净 |

#### Batch 4: 可选清理（工具 / 边缘模块）

```
utils/, logging/, metrics/, tests/, video/, vision/,
suggestion/, clarification/, checkpoint/, ...
```

### 单模块扫荡 SOP

对于每个目录，执行以下 5 步：

```
┌─────────────────────────────────────────────────────────────┐
│                    模块清理 SOP                              │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ① SCAN    查找 FFI 关键字和 .clone() 热点                  │
│     grep -n "FFI\|ffi\|\.clone()" src/<module>/             │
│                                                             │
│  ② AUDIT   检查 pub 导出是否过度                            │
│     grep "^pub fn\|^pub struct\|^pub enum" src/<module>/    │
│                                                             │
│  ③ PRUNE   删除未使用的代码                                 │
│     cargo check 2>&1 | grep "<module>"                      │
│                                                             │
│  ④ TIGHTEN 收紧可见性                                       │
│     pub → pub(crate) → pub(super) → private                │
│                                                             │
│  ⑤ VERIFY  运行测试确保无破坏                               │
│     cargo test -p alephcore -- <module>::                   │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Batch 1 特别注意事项

| 模块 | 特殊处理 |
|------|---------|
| `dispatcher/` | 检查 `async_confirmation.rs` 是否还有 FFI 回调 |
| `gateway/` | 这是新架构核心，只清理而不重构 |
| `memory/` | 注意 `Vec<T>` 克隆，可改为 `&[T]` 切片 |
| `generation/` | 检查是否有 `Box<dyn Fn>` 风格的 FFI 回调 |

## Phase 4: 依赖瘦身

### Step 4.1: 检查 Cargo.toml 中的残留

```bash
# 列出所有依赖
grep -E "^\w+ = " Cargo.toml | cut -d= -f1 | sort

# 检查是否有 FFI 相关的残留依赖
grep -iE "ffi|uniffi|cbindgen|swift|kotlin" Cargo.toml
```

### Step 4.2: 使用 cargo-udeps 检测未使用依赖

```bash
cargo +nightly udeps --all-targets
```

### Step 4.3: 清理 features 门控

检查 `[features]` 部分是否有废弃的 feature：

```toml
# 可能需要删除的：
# ffi = ["uniffi", ...]
# swift-bridge = [...]
```

## 验收标准

| 指标 | 清理前 | 目标 | 验证命令 |
|------|--------|------|----------|
| lib.rs 行数 | 592 | ≤ 350 | `wc -l lib.rs` |
| 编译警告数 | 22 | 0 | `cargo check 2>&1 \| grep -c warning` |
| FFI 关键字出现 | 26 文件 | ≤ 5 | `grep -rl "FFI\|ffi" src/ \| wc -l` |
| pub use 导出 | 44 | ≤ 25 | `grep -c "^pub use" lib.rs` |
| .clone() 热点 | 466 (dispatcher) | ≤ 300 | `grep -rh "\.clone()" dispatcher/ \| wc -l` |

### 最终验证清单

```bash
# 1. 编译通过，无警告
cargo check 2>&1 | grep -E "(error|warning)" && echo "FAIL" || echo "PASS"

# 2. 所有测试通过
cargo test -p alephcore

# 3. Gateway 功能正常
cargo run --features gateway --bin aleph-gateway -- --port 18789 &
# 测试 WebSocket 连接...

# 4. 无 FFI 残留类型
grep -rn "FFI\>" src/ --include="*.rs" | grep -v "// " | wc -l
# 目标：0 或仅剩注释
```

## 避坑指南

### 核验清单

1. **收紧 pub 权限**：重构时很多函数被临时改为了 pub。清理时，如果一个函数只在当前文件夹内使用，立即改为 `pub(crate)`

2. **移除 clippy 坏味道**：由于之前的代码是为了适配 Swift，可能存在大量的 `.clone()` 以规避所有权问题。现在在 Rust 内部，尝试用引用替代这些不必要的拷贝

3. **关注 lib.rs 的腰斩**：理想情况下，清理完成后 lib.rs 的内容应该减少 50% 以上，只保留 gateway 的启动逻辑和核心模块的挂载

### 架构一致性检查

在清理每个 FFI 接口时，问自己：

> "如果这个 FFI 接口被删除了，Swift 端现在是通过哪个 WebSocket Message ID 来请求这个功能的？"

如果发现某个 FFI 接口被删了，但 gateway 模块里还没有对应的消息处理逻辑，那么这就不是简单的"代码清理"，而是"功能补全"。

## 附录：52 个目录完整列表

```
agent_loop/    agents/        bin/           browser/       builtin_tools/
capability/    checkpoint/    clarification/ cli/           clipboard/
command/       components/    compressor/    config/        conversation/
core/          cron/          discovery/     dispatcher/    event/
exec/          executor/      extension/     gateway/       generation/
init_unified/  intent/        logging/       markdown/      mcp/
memory/        metrics/       payload/       permission/    prompt/
providers/     question/      routing/       runtimes/      search/
services/      skills/        suggestion/    supervisor/    tests/
thinker/       thinking/      three_layer/   tool_output/   tools/
utils/         video/         vision/        wizard/
```

---

*Generated with Claude Code*
