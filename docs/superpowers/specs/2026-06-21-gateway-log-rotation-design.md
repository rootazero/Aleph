# gateway.log 体积治理 — 设计文档

- **日期**: 2026-06-21
- **状态**: 已确认，待实现
- **范围**: `shared/logging/src/file_appender.rs`、`src/bin/aleph-server/daemon.rs`

## 1. 问题

`~/.aleph/logs/gateway.log` 已达 82MB 且持续增长、从不轮转，而同目录的
`aleph-server.log.YYYY-MM-DD` 每个都 <3MB。

## 2. 根因诊断

目录下有**两套互相独立**的日志机制：

| 文件 | 来源 | 轮转 |
|------|------|------|
| `aleph-server.log.YYYY-MM-DD` | `tracing` 的 `RollingFileAppender`（`shared/logging`） | ✅ 按天轮转 + 7 天 retention |
| `gateway.log` | daemon 化时 `dup2` 把进程 stdout/stderr 重定向到此文件（`daemon.rs:342`） | ❌ 永久 append |

**关键事实**：`gateway.log` 尾部与 `aleph-server.log.<今天>` 尾部**逐字节相同**。
原因——`setup_logging` 同时挂了 `console_layer`（写 stdout）和 `file_layer`
（写 aleph-server.log）；daemon 把 stdout 重定向进 gateway.log，于是：

```
gateway.log = console_layer 输出（≈ aleph-server.log 的完整副本，约 95%）
            + 真正独占的裸输出（绕过 tracing：bind 失败 eprintln、panic、早期启动错误，约 5%）
```

`gateway.log` 是固定 fd（daemon 化后进程内不可变），既重复又不轮转 → 82MB。

## 3. 决策

**根因方案：停止重复 + 小兜底。** 不把 gateway.log 当完整副本去套轮转，而是从源头
消除重复，让它退化为 KB 级的 panic/早期错误兜底文件，再加一个按大小的启动时滚动 +
复用既有 retention 作为无界增长的兜底。

## 4. 方案

### Part 1 — 从源头消除重复（真正的修复）

`shared/logging/src/file_appender.rs::setup_logging`：**仅当 stdout 是交互式终端时
才挂 console 层**。

```rust
use std::io::IsTerminal;

let console_layer = std::io::stdout().is_terminal().then(|| {
    fmt::layer()
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .event_format(crate::pii_filter::PiiScrubbingFormat)
});

// Option<L> 本身实现了 Layer，registry 链不变：
registry().with(env_filter).with(console_layer).with(file_layer).try_init()
```

**为何自动正确**：`daemonize()` 在 logging 初始化**之前**就重定向了 stdout，因此：

- **daemon 模式** → `is_terminal() == false` → 不挂 console 层 → gateway.log 不再收副本。
- **前台 dev**（`just dev` / `cargo run`）→ stdout 是 TTY → 保留 console 层 → 终端实时可见。
- **桌面壳** → 永远 `--daemon start` + `stdout(Stdio::null())`，从不读 server stdout
  （靠 `/ready` HTTP 探测）→ 零影响。
- 零 API 改动、零调用方改动；一次修好 server / desktop / cli 三端。

**效果**：gateway.log 此后只接真正独占的内容（panic、裸 eprintln、第三方 print），KB 级。

> **迁移提示**：daemon 模式下实时看日志改 `tail -f aleph-server.log.<今天>`
> （内容一致且会轮转），不再 tail gateway.log。

### Part 2 — 与 aleph-server.log 一致的兜底策略

`src/bin/aleph-server/daemon.rs::daemonize()` 在打开 gateway.log 前加**启动时按大小滚动**：

- 新常量 `MAX_GATEWAY_LOG_BYTES = 5 * 1024 * 1024`（5 MiB）。
- 若 `gateway.log` 存在且 `len() > MAX`：`fs::rename` → `gateway.log.YYYY-MM-DD`
  （日期用 `chrono::Utc::now().format("%Y-%m-%d")`——与 `daemon.rs` 既有 `chrono::Utc`
  及 tracing-appender 的 UTC 按天轮转保持一致；10 字符日期后缀正好命中 `cleanup_old_logs`
  的匹配规则）。同日重复滚动覆盖当天备份（兜底文件，可接受）。
- 然后照常 `OpenOptions::append` 打开新的 `gateway.log`。
- 滚动后调一次 `aleph_logging::cleanup_old_logs(&log_dir, 7, Some("gateway"))`
  —— 与 aleph-server.log **同一个** retention 调用，只换前缀，删 7 天前的 `gateway.log.*`。

复用现成机制，无后台线程、无 supervisor（契合 R10 薄 harness / P6 KISS）。

> **已知局限（诚实声明）**：rotate-on-start 只在 daemon (重)启动时触发。连跑数周不重启、
> 又恰好往 stdout 狂写的 daemon，单个 gateway.log 仍可能在两次重启间增长（dup2 fd
> 进程内固定，无法像 tracing-appender 那样午夜自动换文件，除非加后台 re-dup2 线程——
> 本方案为简洁刻意不做）。Part 1 已把写入速率降到近乎为零，折中可接受；将来不够再升级为
> 进程内大小监测线程。

### 存量 82MB 处理

下次 daemon 重启时 >5MiB → 被 rename 成 `gateway.log.<date>` → 7 天后被 retention 清掉。
用户也可现在直接 `rm gateway.log`（daemon 持 fd，删文件名即可，无需停服）。

## 5. retention 匹配约束（命名依据）

`cleanup_old_logs(dir, days, Some("gateway"))` 仅清理：

- `gateway.log`（精确），或
- `gateway.log.<suffix>`，且 `suffix` 正好 10 字符、第 4 与第 7 位是 `-`（即 `YYYY-MM-DD`）。

故兜底备份必须命名为 `gateway.log.YYYY-MM-DD`，不能带时分秒后缀，否则不会被清理。

## 6. 测试

- `daemon.rs`：抽出纯函数 `rotate_oversized_log(path: &Path, max_bytes: u64) -> io::Result<()>`，
  用 tempdir 单测：
  1. 文件 < 阈值 → 不滚动（原文件原样保留）。
  2. 文件 > 阈值 → 被 rename 成 `gateway.log.<date>`，原路径可重新创建。
  3. 文件不存在 → no-op。
- `shared/logging`：现有 init 测试继续通过即可；console 层的 TTY 判定为全局副作用，
  以代码审查为准，不强测全局 subscriber。
- `retention`：`Some("gateway")` 前缀路径已被现有 prefix 测试逻辑覆盖。

## 7. 改动范围

| 文件 | 改动 |
|------|------|
| `shared/logging/src/file_appender.rs` | 条件 console 层（~5 行 + `IsTerminal` import） |
| `src/bin/aleph-server/daemon.rs` | `rotate_oversized_log` 纯函数 + daemonize 调用 + `cleanup_old_logs` 调用 + 常量 + 单测 |

## 8. 红线对齐

- **R10 薄 harness / P6 KISS**：无后台线程、无 supervisor，复用既有 retention。
- **P3 可扩展**：局限点留有清晰的升级路径（进程内大小监测线程）。
- **外科手术**：仅 2 文件，每行变更都可追溯到本需求。
