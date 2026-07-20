# 集群节点能力标签 + 多节点扇出 (Cluster Tags + Fan-out)

> Date: 2026-06-09
> Branch: `feat/cluster-tags-fanout` (worktree `Aleph-wt-cluster-fanout`)
> Scope: gateway 集群子系统的能力扩展（A: 节点标签；B: 多节点 scatter-gather）

## 背景与 Gap Analysis

参考项目对齐（中心驱动远程执行节点维度）：

- **hermes-agent** 不是多机集群（单机 gateway + 多渠道 + ACP + cron + subagent），不构成集群参照系。
- **openclaw** 是唯一相关参照：有节点能力标签（`declaredCaps`/`caps`/`permissions`）、按标签/平台的命令策略、`checkConnectivity` 健康探测、TaskRegistry 任务状态机。

Aleph 集群子系统经前几轮会话（reverse-RPC / registry / file-transfer / approval / node-liveness）已**全部接通且零 dead code**。本轮真实重心**不是修 bug / 连线，而是能力扩展**。最高价值且不碰红线的缺口：

| 缺口 | openclaw | Aleph 现状 | 本轮 |
|---|---|---|---|
| 节点能力标签 | `declaredCaps`/`caps` | ❌ 仅 `declared_commands` | **A 实现** |
| 能力/标签路由 | 标签选择 | ❌ 只能按 name/id 点名 | **B 实现** |
| 多节点扇出 scatter-gather | ❌（单节点） | ❌ | **B 超越**（Tokio 并发） |

多节点扇出是 openclaw / hermes 共同空白，用 Tokio `JoinSet` 并发执行即为 Aleph 的 Rust 超越点。

## 红线归属

- **R1**：节点仍是执行臂，标签不改变其执行边界。
- **R4**：`Environment` 新增 `tags` 不含凭证，仍是薄渲染契约。
- **R7**：标签**纯粹用于选择**，不构成新授权层；命令执行权威仍是节点侧 `CommandTable` allowlist；选择哪些节点、跑什么命令由中心 LLM 决定，集群层只做确定性查表/并发分发，无推理。
- **R10**：集群代码不进入 `src/harness/`，本轮零 harness 改动。
- **R3 / P6**：刻意不引入 openclaw 的 `caps` 独立能力分类法；`declared_commands` 已充当功能能力清单，`tags` 只做运维分组标签，单一 `Vec<String>` 足够。

## A. 数据模型 — 节点标签

给节点加一层**自由文本标签** `tags: Vec<String>`（如 `gpu`、`region=us`、`role=builder`）。

- bareword（`gpu`）与 `key=value`（`region=us`）都按**整串字符串**存储与匹配，**不解析 kv**，保持最简。
- 标签纯用于选择，不参与授权。

### 落点（连线优先，复用现有结构）

| 结构 | 改动 | 文件 |
|---|---|---|
| `NodeSession` | 增 `tags: Vec<String>` | `src/cluster/registry.rs` |
| `Environment`（投影契约） | 增 `tags: Vec<String>`（`environments.list` 自动可见，无新 RPC） | `src/cluster/registry.rs` |
| `maybe_register_node` | 接收并存 `tags` | `src/cluster/registry.rs` |
| `connect` 帧 | 在现有 `commands` 旁带 `tags` | `src/gateway/handlers/...`（connect 解析处） |
### 标签来源 — CLI 声明（每次启动提供，不持久化）

- `aleph-server node --tag gpu --tag region=us ...`（`--tag` 可重复）。
- **实现期决定（偏离初稿）**：tags 由 CLI flag **每次启动提供**，经 `connect` 帧上报，**不**写入 `~/.aleph/node/<name>.json` 凭证文件、**不**走 `pairing.start_node` 帧。
  - 理由：凭证文件只承载认证身份（`node_id`/`bearer`/`center`）；把 tags 也塞进去会产生"flag 与磁盘值谁优先"的歧义。CLI-每次提供与现有 `--name`/`--center` 的供给模型一致。
  - 中心只在 `connect`（配对完成后的正式连接）落 `NodeSession`，故 tags 经 connect 帧即完全连通；`pairing.start_node` 处中心不消费 tags，发过去是死字段，故不发（避免 advertised-but-unwired）。
  - 取舍：若节点重启时漏传 `--tag`，会以空 tags 重新登记并掉出 tag 扇出。由 launch 命令（systemd/launchd）保证 argv 完整。

## B. 寻址扩展 + 多节点扇出

### `NodeRegistry::resolve_all_by_tags`

`registry.rs` 新增，与现有 `resolve` 并列，复用同一把锁/内部表：

```rust
/// 返回所有【在线且含全部请求 tag】的节点。空 tags = 全部在线节点。
pub fn resolve_all_by_tags(&self, tags: &[String]) -> Vec<NodeMatch>;

pub struct NodeMatch {
    pub node_id: String,
    pub name: String,
    pub channel: ReverseRpcChannel,
    pub declared_commands: Vec<CommandDescriptor>,
}
```

- **AND 语义**：节点必须含**全部**请求 tag 才入选。
- 仅在线会话（registry 本就只存在线节点）。
- 空 tag 列表 = 匹配全部在线节点（"对所有节点广播"的合法用法）。

### 中心侧 LLM 工具 `node_invoke_many`

新文件 `src/builtin_tools/node_invoke_many.rs`。与 `node_invoke` 语义**显式分离**：`node_invoke` 保持"解析→唯一节点，歧义=报错"；`node_invoke_many` 是"selector 匹配一组节点并发跑"。

入参：

```jsonc
{ "tags": ["gpu"],            // AND 匹配；[] = 所有在线节点
  "command": "bash",          // 每个命中节点都要声明该命令
  "args": { "cmd": "nvidia-smi -L" },
  "timeout_ms": 120000 }      // 每节点独立超时，默认 120000
```

行为：

1. `resolve_all_by_tags(tags)` 取命中集合。
2. **零命中报错**：无在线节点匹配 → 返回明确错误 + 可用标签提示（镜像 `resolve` 的 fail-fast 风格），让 LLM 立即纠正而非静默空跑。
3. **并发执行**：`tokio::task::JoinSet` 对所有命中节点并发 `channel.call("tool.call", {tool: command, args})`；墙钟 = 最慢单节点。
4. **逐节点 fail-fast**：复用 `node_invoke` 的"节点声明非空命令目录却不含该命令 → 拒"检查，逐节点判定（不阻塞其他节点）。
5. **容忍部分失败**：单节点超时/错误不拖垮整次调用。

返回聚合：

```jsonc
{ "invoked": 3, "succeeded": 2, "failed": 1,
  "results": [
    { "node": "gpu-1", "node_id": "…", "ok": true,  "result": {…} },
    { "node": "gpu-2", "node_id": "…", "ok": true,  "result": {…} },
    { "node": "gpu-3", "node_id": "…", "ok": false, "error": "reverse-rpc timeout after 120000ms" }
  ] }
```

- `results` 顺序不保证（并发完成顺序），每项含 `node`/`node_id` 供 LLM 对齐。

### 工具注册 + 权限

- `node_invoke_many` 走与 `node_invoke` 相同的 builtin_tools 注册路径，要求 caller 有相同的集群操作能力。
- `environments.list` 的 authz 不变（read 只读）；新增 `tags` 字段不含凭证（R4）。

## 熵减 / 清理

- **诚实说明**：集群子系统当前无 dead code 可删，本任务实质为纯能力扩展。
- 唯一"连线"动作：把新 `tags` 字段从 `CLI → connect/pairing → maybe_register_node → NodeSession → Environment` 逐跳贯通，避免"已造未连"半接线。
- 实现期若发现真实边缘 bug 就地标注修复，不臆造。

## 测试计划

集群不在 harness，R10 不涉及。

- `registry.rs`：
  - `resolve_all_by_tags` AND 命中（含全部 tag）
  - 部分 tag 不命中 → 排除
  - 空 tag → 全部在线节点
  - 离线节点不入选
- `node_invoke_many.rs`：
  - 扇出聚合计数（invoked/succeeded/failed 一致）
  - 部分失败容忍（一个节点超时，其余成功仍返回）
  - 逐节点 undeclared-command fail-fast
  - 零命中报错 + 标签提示
- `node.rs`：
  - `--tag` bareword 与 `key=value` 解析
  - 凭证持久化往返（写盘后回读含 tags）

## 受影响文件

| 文件 | 改动 |
|---|---|
| `src/cluster/registry.rs` | `NodeSession.tags` + `Environment.tags` + `resolve_all_by_tags` + `NodeMatch` |
| `src/cluster/mod.rs` | 导出 `NodeMatch`（若需） |
| `src/builtin_tools/node_invoke_many.rs` | **新文件**：扇出工具 |
| `src/builtin_tools/mod.rs`（或注册处） | 注册 `node_invoke_many` |
| `src/gateway/handlers/cluster.rs`（及 connect/pairing 解析处） | 透传 `tags` |
| `src/bin/aleph-server/commands/node.rs` | `--tag` 解析 + 持久化 + connect/pairing 上报 |
| `docs/reference/CLUSTER.md` | 文档化 tags + `node_invoke_many` |

## 交付协议

- 分支隔离：worktree `Aleph-wt-cluster-fanout` / 分支 `feat/cluster-tags-fanout`，从 main 切。
- **完成后不跑 cargo check 测试校验，直接提交**（资源并发治理强制约束）。
- 留分支不碰 main，合并由用户管理。

## 非目标（DEFERRED）

- C: 主动健康探测（`node.ping` 往返延迟）+ 节点级可观测（命令计数/成功率）。
- 任务队列状态机、流式输出、命令 schema 校验。
- 标签的 kv 解析 / 范围查询（如 `region=*`）。
