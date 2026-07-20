# Aleph 集群文件传输（node ↔ center）设计

> 子项目：集群 0c-pairing 后续三子系统之 **②**（① 多命令被本子系统吸收，③ 节点侧审批路由回中心另立 spec）。
> 前置：main 已含 0a（反向 RPC）+ 0b（NodeRegistry）+ 0c-core（NodeClient/node_invoke/CommandTable）+ 0c-pairing（交互式 enroll）。

**Goal:** 让 LLM 在中心与某个已连节点之间双向传输文件，字节走 0a 反向 RPC，永不进入 LLM 上下文。

**Architecture:** 中心侧新增 `node_file` LLM 工具（按路径编排 push/pull）；节点侧新增 `file.read`/`file.write` 两个 `NodeCommand`（直接 host-fs，jail 在节点 session workspace）。单帧、硬 8MB 上限、两端 sha256 完整性校验、fail-fast。

**Tech Stack:** Rust（alephcore）、async-trait、serde_json、base64、sha2、`file_ops::path_utils::check_and_resolve_path`、0a `ReverseRpcChannel`。

---

## 1. 动机与边界

### 1.1 为什么需要专用工具（而非 LLM 串 file_ops + node_invoke）
朴素方案是让 LLM 读中心文件（`file_ops`）再喂给 `node_invoke`。**致命缺陷**：字节会途经模型上下文窗口——一个 8MB 文件 ≈ 11MB base64 文本进 prompt，违背传输的全部意义。因此必须有专用中心工具，让字节在**中心进程 ↔ 节点进程**间流动，LLM 只传路径。这正是 R7/R8 正确切分：LLM 决定**意图**（哪个文件、哪个节点、哪个方向），系统搬**字节**。

### 1.2 ① 多命令被吸收
现有线协议（`node_invoke` 把 `command`/`args` verbatim 透传 + `CommandTable` keys=allowlist）**本就多命令**。bash 已覆盖一切 shell 可表达操作（`ls`/`cat`/`ps`/`uname`）。bash 做不好的只有二进制/大文件传输——即本子系统。故 ① 无残留范围，吸收入 ②。

### 1.3 v1 范围（YAGNI）
- **双向**：`file.read`（pull，节点→中心）+ `file.write`（push，中心→节点）。
- **单帧 + 硬 8MB 上限**：不分块。WS 默认单帧 ~16MiB，base64 膨胀 ~33% 后 8MB 原始字节安全落在单帧内。超限 fail-fast。
- **不做**：分块/断点续传（>8MB 真出现时另立 spec）、目录递归传输、流式、压缩、节点间直传。

---

## 2. 组件与文件

| 文件 | 职责 | 改动 |
|------|------|------|
| `src/cluster/node_file_cmd.rs` | **新建**。节点侧 `FileReadCommand`/`FileWriteCommand` impl `NodeCommand`：path jail、8MB 上限、base64 编解码、sha256 校验、overwrite 保护 | 建 |
| `src/cluster/node_runtime.rs` | `CommandTable` 增 `register_file_commands(workspace_root, session)` 便捷构造；`mod.rs` 导出新命令 | 改 |
| `src/cluster/mod.rs` | `pub mod node_file_cmd;` + re-export | 改 |
| `src/builtin_tools/node_file.rs` | **新建**。中心侧 `node_file` LLM 工具 | 建 |
| `src/builtin_tools/mod.rs` | `pub mod node_file;` + re-export `NodeFileTool`/`NodeFileArgs` | 改 |
| `src/bin/aleph-server/commands/node.rs` | `build_command_table` 注册 file 命令（与 bash 同 workspace_root + session） | 改 |
| `src/executor/builtin_registry/definitions.rs` | `node_file` 工具 definition | 改 |
| `src/executor/builtin_registry/registry.rs` | `node_file` OnceCell + setter + dispatch arm（镜像 node_invoke） | 改 |
| `src/executor/builtin_registry/groups.rs` | `node_file` 归入 `cluster` 组 | 改 |
| `src/executor/builtin_registry/builder/optional_tools.rs` | `node_file` optional 注册 + schema | 改 |
| `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` | boot 注入 `node_file`（与 node_invoke 同处，无条件） | 改 |
| `tests/cluster_node_runtime.rs` | 集成测试：`file.write`→`file.read` byte-identical 往返 | 改 |

---

## 3. 节点侧命令（`node_file_cmd.rs`）

### 3.1 常量与共用
```rust
/// 单文件硬上限（原始字节）。两端一致。超过即 fail-fast。
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
```

### 3.2 path jail
节点 file 命令解析 `path` 时，**复用** `file_ops::path_utils::check_and_resolve_path`，以**节点 session workspace 目录**作为 `output_dir_override`（即 jail root），`denied_paths = get_denied_paths()`。这样：
- 相对路径 resolve 到 session workspace；
- `..`/symlink traversal 被 canonicalize 逻辑拦截；
- 与 bash 命令**同一** workspace（push 的脚本可被 bash 直接跑）。

session workspace 目录 = `WorkspaceSandbox` 为该 `SessionKey` 创建的 session 子目录。节点构造时已知 `workspace_root`（`SandboxConfig::default().workspace_root`）与 `SessionKey`；命令持有 `workspace_dir: PathBuf`（构造期算出并 `create_dir_all`，与 bash session dir 同根同名）。

> **实现细节（plan 须定死）**：session 子目录命名须与 `WorkspaceSandbox` 内部一致。若 `WorkspaceSandbox` 不暴露该路径派生函数，则节点 file 命令与 bash 共享一个**显式传入**的 `workspace_dir`，并在 `build_command_table` 处一次性算出，bash 与 file 命令都用它——保证一致而不耦合 sandbox 内部命名。

### 3.3 `FileWriteCommand`（push 落地）
args：`{ "path": str, "content_b64": str, "sha256": str, "overwrite": bool=false }`
1. base64 decode `content_b64` → bytes；decode 失败 → `Err("file.write: invalid base64")`。
2. `bytes.len() > MAX_FILE_BYTES` → `Err("file.write: exceeds 8MB cap")`。
3. 算 `sha256(bytes)`，与 args.sha256 比对，不匹配 → `Err("file.write: sha256 mismatch")`。
4. `check_and_resolve_path(path, &denied, Some(&workspace_dir))` → canonical 落点；越界 → `Err`。
5. 落点已存在且 `!overwrite` → `Err("file.write: target exists (set overwrite)")`。
6. `std::fs::write(canonical, &bytes)`；建父目录。
7. 返回 `Ok({ "written": bytes.len() })`。

descriptor：`{ name:"file.write", schema:{type:object, ...} }`。

### 3.4 `FileReadCommand`（pull 取出）
args：`{ "path": str }`
1. `check_and_resolve_path(path, &denied, Some(&workspace_dir))`；越界 → `Err`。
2. 不存在 → `Err("file.read: not found")`。
3. `std::fs::metadata().len() > MAX_FILE_BYTES` → `Err("file.read: exceeds 8MB cap")`（读前先看大小，避免 OOM）。
4. `std::fs::read(canonical)` → bytes。
5. 返回 `Ok({ "content_b64": base64(bytes), "sha256": hex, "size": bytes.len() })`。

descriptor：`{ name:"file.read", schema:{type:object, ...} }`。

### 3.5 注册
```rust
impl CommandTable {
    /// 在已有 bash 之外注册 file.read/file.write（共享 workspace_dir）。
    pub fn register_file_commands(&mut self, workspace_dir: PathBuf) {
        self.register("file.read", Arc::new(FileReadCommand::new(workspace_dir.clone())));
        self.register("file.write", Arc::new(FileWriteCommand::new(workspace_dir)));
    }
}
```
`build_command_table`（node.rs）：算出 `workspace_dir`（workspace_root + session 派生）→ `with_bash` → `register_file_commands(workspace_dir)`。

---

## 4. 中心侧工具（`node_file.rs`）

### 4.1 Args
```rust
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NodeFileArgs {
    /// Target node: name or id (see `environments.list`).
    pub node: String,
    /// "push" (center→node) or "pull" (node→center).
    pub direction: String,
    /// Center-side path (source for push, destination for pull).
    pub local_path: String,
    /// Node-side path (destination for push, source for pull).
    pub remote_path: String,
    /// Overwrite an existing destination. Default false.
    #[serde(default)]
    pub overwrite: bool,
    /// Reverse-RPC timeout ms (default 120000).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}
```

### 4.2 行为
持有 `Arc<NodeRegistry>`（同 `NodeInvokeTool`）。`resolve(&node)` 拿 `(channel, declared)`；离线 → `Err`。fail-fast 校验节点声明含所需命令（push 需 `file.write`，pull 需 `file.read`；`declared` 非空且不含即拒）。

**push：**
1. `check_and_resolve_path(local_path, &denied, output_dir)` 读中心盘；中心 deny-list/越界 → `Err`。
2. `bytes.len() > MAX_FILE_BYTES` → `Err`。
3. base64 + sha256。
4. `channel.call("tool.call", {tool:"file.write", args:{path:remote_path, content_b64, sha256, overwrite}}, timeout)`。
5. 节点 error → 透传；成功 → 返回 `{ "direction":"push", "bytes":N, "sha256":hex, "local_path", "remote_path" }`。

**pull：**
1. `channel.call("tool.call", {tool:"file.read", args:{path:remote_path}}, timeout)`。
2. 节点 error → 透传；成功取 `content_b64`/`sha256`/`size`。
3. base64 decode；本地算 sha256 比对节点 sha256，不匹配 → `Err("sha256 mismatch in transit")`。
4. `len > MAX_FILE_BYTES` → `Err`（防节点绕过）。
5. `check_and_resolve_path(local_path, ...)`；落点已存在且 `!overwrite` → `Err`。
6. `std::fs::write` 落中心盘。
7. 返回 `{ "direction":"pull", "bytes":N, "sha256":hex, "local_path", "remote_path" }`。

`direction` 非 push/pull → `Err("direction must be 'push' or 'pull'")`。

### 4.3 中心 path 安全的 `output_dir` 来源
`check_and_resolve_path` 第三参 `output_dir_override` 决定中心 jail root。本工具运行在中心 daemon、无 ToolContext session workspace。决策：传 `None`，依赖 deny-list（`/etc/passwd` 等）拦截敏感路径，其余中心盘 operator 可达——与"中心侧复用 file_ops 路径安全"决策一致（防御 deny-list 而非强 workspace jail，因中心传输本就是 operator 主动行为）。
> 若后续要求强 jail，可加配置项 `cluster.file_transfer.center_root`；v1 不做（YAGNI）。

### 4.4 DESCRIPTION（节选要点）
说明：按路径在中心与节点间传输文件；字节不进对话；`direction` push/pull；超 8MB 报错；目标存在需 `overwrite`；节点须声明 `file.read`/`file.write`。

---

## 5. 数据流（端到端）

```
PUSH  LLM ─node_file{push,/c/x.sh,w1,/tmp/x.sh}─▶ 中心工具
        read /c/x.sh (path-safe, ≤8MB) ─▶ base64+sha256
        ─reverse RPC tool.call{file.write,path,content_b64,sha256,overwrite}─▶ 节点
        节点: b64 decode → ≤8MB → sha256 校验 → jail path → overwrite 检查 → fs::write
        ◀─{written:N}── 中心 ─▶ LLM {bytes,sha256,paths}

PULL  LLM ─node_file{pull,w1,/tmp/out.bin,/c/out.bin}─▶ 中心工具
        ─reverse RPC tool.call{file.read,path}─▶ 节点
        节点: jail path → exists → ≤8MB → fs::read → base64+sha256
        ◀─{content_b64,sha256,size}── 中心: decode → sha256 校验 → ≤8MB → write /c/out.bin
        ─▶ LLM {bytes,sha256,paths}
```

---

## 6. 错误与安全（P7 防御性）

| 风险 | 防御 |
|------|------|
| 大文件 OOM | 两端 8MB 硬上限；pull 读前先查 metadata 大小 |
| 路径遍历/symlink | 两端 `check_and_resolve_path`（canonicalize + deny-list） |
| 传输损坏 | 两端 sha256，不匹配即丢弃报错 |
| 误覆盖 | `overwrite` 默认 false，目标存在即拒 |
| 节点越权读 | 节点 jail 在 session workspace；allowlist 不注册即整体禁用 |
| 部分写 | 校验全部前置，`fs::write` 原子整写，绝不流式部分落盘 |
| 中心敏感路径 | deny-list（`/etc/passwd` 等） |

**红线合规**：R1（节点纯执行臂，直接 fs 合理，无平台 API）/R4（中心工具纯 I/O 翻译，无业务逻辑）/R7（LLM 只按路径编排，不搬字节、不做内容判断）/R10（`src/harness/` 零改动）。

---

## 7. 测试

### 7.1 节点命令单测（`node_file_cmd.rs` inline，tempdir 作 workspace_dir）
- `file_write_then_read_round_trips`：write base64 → read → byte-identical。
- `file_write_rejects_oversize`：>8MB → Err。
- `file_write_rejects_sha_mismatch`：错 sha256 → Err。
- `file_write_rejects_traversal`：`../../etc/x` → Err（jail）。
- `file_write_respects_overwrite`：已存在 + overwrite=false → Err；overwrite=true → Ok。
- `file_read_rejects_missing`：不存在 → Err。
- `file_read_rejects_oversize`：构造 >8MB 文件 → Err。

### 7.2 中心工具单测（`node_file.rs`，mock 反向 channel，镜像 node_invoke 测试）
- `push_sends_write_with_sha_and_returns_summary`。
- `pull_writes_local_and_verifies_sha`。
- `push_rejects_oversize_local`。
- `pull_rejects_sha_mismatch_in_transit`。
- `rejects_unknown_direction`。
- `rejects_command_not_declared_by_node`。

### 7.3 集成（`tests/cluster_node_runtime.rs` 扩）
- `command_table_file_roundtrip`：CommandTable dispatch `file.write{content_b64}` → dispatch `file.read` → 解出 byte-identical 原文。

---

## 8. 实现顺序（plan 将据此 TDD 拆任务）

1. **Task 1 — 节点 file 命令**：`node_file_cmd.rs`（MAX_FILE_BYTES、FileReadCommand、FileWriteCommand、jail、sha256）+ `register_file_commands` + 单测（§7.1）。
2. **Task 2 — 中心 node_file 工具**：`node_file.rs`（NodeFileArgs、push/pull、path 安全、sha256 校验）+ 单测（§7.2）。
3. **Task 3 — 接线与集成**：node.rs build_command_table 注册 file 命令 + builtin_registry 6 处 + agent_init 注入 + 集成测试（§7.3）。

每任务 implementer → spec 审查 → code quality 审查；末了整体审查。新 worktree 从 main 切（main 已含 0c-pairing）。**不合 main**（用户管理 cluster 合并策略）。

---

## 9. 后续（本 spec 不含）
- 分块/断点续传（>8MB）。
- 目录递归传输。
- 子项目 ③：节点侧审批路由回中心（另立 spec）。
