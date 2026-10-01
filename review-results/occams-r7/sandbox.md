# sandbox review — 2026-10-01

## 概览
- 模块规模：60 文件，约 22,483 行
- 角色（来自 graph.json）：安全沙箱核心——能力/策略/命令策略/进程代理/网络代理/cgroup 资源控制/DNS 沙箱/平台分支/worktree 隔离。
- 估计节省：~50 行（低价值模块，代码质量已较高）

## P0 — 安全/阻塞（无）

## P1 — 逻辑/正确（高置信度）

- **`src/sandbox/exec_approval/denial_ledger.rs:493–498` — `record_denial` 中 `session.to_string()` 重复调用**
  - 现象：`session` 在同一函数作用域内调用了 4 次 `.to_string()`，每次都分配新的 `String`。
  - 证据：
    ```rust
    // 行 493
    guard.order.push_back(session.to_string());  // 第1次
    }
    let denials = guard.by_session.entry(session.to_string()).or_default();  // 第2次
    // 行 498
    *denials.counts.entry(fingerprint.to_string()).or_insert(0) += 1;  // fingerprint 也要优化
    ```
  - 建议：提取到顶部 `let session_key = session.to_string();` 并复用，节省 3 次堆分配。

- **`src/sandbox/workspace/mod.rs:223–235` — `requested_cwd` 同一路径双重克隆**
  - 现象：`cmd.cwd.is_none()` 分支中 `requested_cwd` 已在行 227 克隆过一次，行 232 又对同一值 `requested_cwd.clone()`。
  - 证据：
    ```rust
    let requested_cwd = match &cmd.cwd {
        None => ws.cwd.clone(),       // 克隆 #1
        Some(p) => normalize_path(p, &ws.cwd),
    };
    let mut cwd = if cmd.cwd.is_none() {
        // rust-doctor-disable-next-line excessive-clone
        requested_cwd.clone()          // 克隆 #2（对同一个 String）
    } else { ... }
    ```
  - 建议：分支内直接 `requested_cwd`（所有权已转移），删去第二个 `clone()`。rust-doctor-disable-next-line 也随之移除。

## P2 — 复杂度/错误处理

- **`src/sandbox/exec_approval/denial_ledger.rs:513` — `probe_failed` 逻辑标注为"今日冗余"但未被删除**
  - 现象：`probe_failed = denials.state == PauseState::HalfOpen;` 紧接其后有注释承认："Redundant *today*: nothing reaches `HalfOpen` without leaving `consecutive` at or above the threshold... Kept because it states the property we actually mean."
  - 证据：`denial_ledger.rs:506–511` 注释明确说明在当前代码路径下该分支永远为 `false`，但没有删除或添加 `#[cfg(test)]` 以保留测试价值。
  - 建议：要么（a）删除该分支并加上 `// Safety net: kept in case future code paths introduce HalfOpen without reaching threshold`，或（b）通过 `cargo hack --propagate-versions` 验证是否可删。当前以保留为佳，但注释应标记为 `// ALPHA: remove after confirming no new HalfOpen entry point`。

- **`src/sandbox/workspace/mod.rs:206` — `execute` 函数标注高复杂度禁用，但未拆分**
  - 现象：`// rust-doctor-disable-next-line high-cyclomatic-complexity` 保护了一个 ~100 行的 async 函数，内部有 6 步 pipeline（cwd 解析、containment check、capability escalation、profile_for、run、denial hint）。
  - 证据：`workspace/mod.rs:205–206`。
  - 建议：当前设计可接受（execute 是自然的 entry point 边界），但若未来超过 ~150 行应考虑拆分 approval/revalidation 子逻辑。已记录为架构债务，不要求立即修改。

- **`src/sandbox/platforms/` — 4 个 `#\[allow(clippy::too_many_arguments)\]` 分散在跨平台驱动中**
  - 现象：`driver.rs:73`、`linux/bwrap.rs:547`、`windows/driver.rs:246`、`macos/seatbelt.rs:968` 均对各自的 `run()` trait 方法添加了该允许。
  - 证据：grep 结果，无例外。
  - 建议：当前可接受（`OsSandboxDriverTrait::run` 签名本身携带平台上下文，且 Rust 的 trait 对象约束使参数数量受业务约束驱动）。如未来 Rust 支持负参数数量 lints，可统一在 trait 签名处允许一次。

## P3 — 性能/风格（仅总数）
- 4 条（均为中低优先级，见 P1/P2）

## 误报自检

以下曾考虑但放弃：

- **`exec_approval/denial_ledger.rs:541,553,566` 的 `#[cfg(test)]` 方法是否算死代码**：经确认它们都在 `mod tests` 中被直接调用（`led.denial_count()`, `led.consecutive()`, `led.expire_cooldown()`），是 `#[cfg(test)]` 条件编译而非跨模块死代码——保留。

- **`cgroup_v2.rs` 的 `#![cfg_attr(not(target_os = "linux"), allow(dead_code))]` 是否掩盖了死代码**：经确认 `cgroup_v2.rs` 是 Linux-only 资源控制（SP-5），其跨平台存根（`parse_proc_self_cgroup_path`、`cpu_quota_max_line` 等）在 Linux 上全部被 `linux/bwrap.rs:673–698` 使用，非死代码。

- **`normalize.rs` 的 `append_views` 是否有不必要分配**：`MAX_VIEW_BYTES = 2MB` cap + `MAX_DECODED_PAYLOADS = 8` + `MAX_DECODE_ROUNDS = 2` 的级联限制在语义上是有意的延迟上限，不是过度分配。

## 未审查清单

以下文件/子目录已读但未深入审计其内部逻辑，或完全未读——明示盲区：

| 文件 | 原因 |
|------|------|
| `command_policy/normalize.rs:201–866`（append_views 正文） | 逻辑复杂但测试覆盖率很高（~200 行测试），高置信度无明显缺陷 |
| `exec_approval/grants.rs`（883 行） | 仅扫描了 re-export 和签名；持久化/原子写入逻辑未逐行审计 |
| `sandbox_init.rs:21–1383`（Linux init 逻辑） | 平台分支复杂，依赖 bwrap syscall，测试覆盖有限但审计线索无异常 |
| `platforms/linux/bwrap.rs`（1114 行） | 平台特有逻辑量大；仅抽检了 cgroup 集成段 |
| `platforms/windows/driver.rs`、`platforms/windows/job.rs` | Windows 特化代码，worktree 为 Linux，未运行 Windows 构建验证 |
| `platforms/macos/seatbelt.rs` | macOS 特化代码，仅抽检 |
| `command_policy/config.rs`、`command_policy/normalize.rs` | 规则配置和规范化逻辑已读核心段 |
| `workspace/path.rs`、`workspace/env.rs`、`workspace/approval.rs`、`workspace/proxy.rs` | 子模块较薄（80–140 行），无明显信号 |
| `proxy/socks5.rs`、`proxy/netns_bridge.rs`、`proxy/dial.rs` | 网络代理逻辑已读核心段 |
| `windows_init/imp/app_container.rs` | Windows AppContainer 实现，未在 Linux 构建中验证 |