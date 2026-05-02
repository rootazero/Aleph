---
title: Spec C — Cross-Process Safety Beyond Curated Layer
date: 2026-05-02
status: approved
owner: @user
related_refs:
  - docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md
  - docs/superpowers/specs/2026-05-01-memory-evolution-spec-a-curated-hot-snapshot-design.md
  - docs/superpowers/specs/2026-05-01-memory-evolution-spec-b-session-search-summarization-design.md
  - docs/reference/SECURITY.md
  - CLAUDE.md (process management section)
---

# Spec C — Cross-Process Safety Beyond Curated Layer

> Memory Evolution Roadmap 第三个 follow-up spec。Spec A 关掉了 `MEMORY.md` 的跨进程写竞争，Spec B 关掉了 `session_search` 的语义层。Spec C 关掉**剩余所有跨进程写入面**：CLI bypass、未加锁的 SQLite 库、裸文件、singleton 的鲁棒性洞。

## 1. 背景与动机

### 1.1 已有保护

| 保护 | 来源 | 覆盖面 |
|---|---|---|
| `aleph.lock` singleton (`flock(LOCK_EX\|LOCK_NB)`) | 现有 `src/bin/aleph-server/daemon.rs::acquire_instance_lock` | server 启动时阻止第二个 server 起来 |
| `MEMORY.md` fcntl + atomic temp+rename | Spec A `src/memory/curated/format.rs` | curated hot memory 写路径 |
| WAL+busy_timeout=5000 | `memory.db` / `cron.db` / `heartbeat.db` / `tasks/shared` | 这 4 个 DB 的并发读 |

### 1.2 真实缺口

CLAUDE.md 已警告："多个 aleph 进程同时运行会竞争写入 ... 导致 vault 数据永久丢失。" 现状下保护 incomplete 在四处：

| # | 缺口 | 后果 |
|---|---|---|
| G-c-1 | CLI 子命令（`aleph secret set` 等）直接打开 `vault.db` / `security.db`，不走 server，不竞 singleton 锁 | 与 server 并发时 vault corruption |
| G-c-2 | ~12 个 `.db` 文件没启用 WAL + busy_timeout | 一旦 singleton 失效，并发读阻塞写、写崩在 SQLITE_BUSY |
| G-c-3 | `secrets.vault` / `acp_sessions.json` 裸 `fs::write`，无 atomic、无 fcntl | 写中段断电/SIGKILL → 半写文件、Aleph 启动失败 |
| G-c-4 | `aleph.lock` 获取时机偏晚（tracing init / config load 之后），错误模型扁平，CLI 无法结构化分流，Windows fallback 不真锁 | 鲁棒性 + 平台一致性双坑 |

Spec C 把这 4 处一次性闭合。

### 1.3 与 Spec A/B 的关系

- **Spec A** 已抽 `fs2` 依赖、已在 curated layer 用 fcntl + atomic write。Spec C 复用其 dependency，**不动**其内部实现。
- **Spec B** 已落地 `INSERT OR IGNORE` + 部分 unique index 模式，证明 SQLite + WAL 在 Aleph 内可行。Spec C 把 WAL 推广到所有 `.db` 文件。

---

## 2. 目标 + 不变量

### 2.1 目标

**单一目标**：在任何执行路径下，`~/.aleph/data/` 的写入只能由**唯一一个进程**进行；当用户用 CLI 执行特权操作时，要么自动接力到运行中的 server，要么本地竞争同一把锁，**不存在两个进程并发写同一存储的窗口**。

### 2.2 不变量（spec 验收的硬约束）

1. **Singleton 唯一性**：在 `~/.aleph/data/aleph.lock` 上以 `flock(LOCK_EX|LOCK_NB)` 持锁的进程是当前 data_dir 的**唯一持久写者**。
2. **早期获取**：lock 在 server start 路径上**早于**任何 `~/.aleph/data/` 下文件/DB 的打开。
3. **CLI 协议**：所有 CLI 子命令在执行前进入决策树 — 先 `try_lock` → 失败时按命令类别分流（IPC / 拒绝）。
4. **Vault 双层防护**：`secrets.vault` 写路径 = atomic temp+rename **AND** 邻接 `.lock` 文件 fcntl 串行化。即使 singleton 被异常绕过仍能保数据完整。
5. **SQLite 一致性**：所有 `~/.aleph/data/*.db` 在打开时强制走 `open_sqlite_safe(path)` helper（WAL + `busy_timeout=5000` + `synchronous=NORMAL` + `foreign_keys=ON`）。
6. **静态可证**：单元/集成/proptest 至少覆盖 — (a) 双 server 并发启动只活一个；(b) CLI 在 server 跑时正确 IPC/拒绝；(c) vault atomic write crash-safe；(d) sqlite 并发读不阻塞。

### 2.3 显式排除（YAGNI 边界）

- **不**做 NFS/分布式文件锁兼容：Aleph data_dir 永远本地。
- **不**做 SQLite migration 锁：singleton 已物理保证单写者。
- **不**做 retry/backoff on `SQLITE_BUSY`：busy_timeout 已给 5s 窗口，超过即运维问题。
- **不**给 `sessions/`、`transcripts/` 子目录加 fcntl：单 server + WAL 已足够。
- **不**改 `MEMORY.md` 路径：Spec A 已定型。
- **不**做 read-only "shadow CLI"（CLI 在 server 跑时只能读不能写）：太复杂，B-thin 走拒绝即可。
- **不**修改 SQLite cipher / encryption 配置：与 Spec C 范围正交。

---

## 3. 单进程锁加固 (G-c-4)

### 3.1 现状缺陷

`acquire_instance_lock()` 在 `src/bin/aleph-server/daemon.rs`，存在 4 个问题：

1. **位置错**：在 binary crate 内，CLI 子命令无法复用。
2. **路径硬编码**：用 `dirs::home_dir().join(".aleph/data/aleph.lock")`，测试无法用 tempdir 替换。
3. **错误模型扁平**：成功返回 `File`、失败返回 `Box<dyn Error>` 含人类可读字符串 → CLI 无法结构化分流。
4. **获取时机偏晚**：在 tracing init / config load 之后才取锁。
5. **Windows fallback 不真锁**：当前 `cfg(not(unix))` 路径只写 PID 不上锁。

### 3.2 模块迁移

从 `src/bin/aleph-server/daemon.rs` 抽到 `src/utils/instance_lock.rs`（core crate），同时支持 server + CLI。`daemon.rs` 仅留薄 re-export 防外部导入断裂。

### 3.3 API（结构化）

```rust
pub struct InstanceLock {
    file: std::fs::File,
    path: PathBuf,
    holder_pid: u32,
}

impl Drop for InstanceLock {
    // OS automatically releases flock on file drop / process exit.
}

pub enum AcquireOutcome {
    Acquired(InstanceLock),
    HeldByLive { pid: i32, lock_path: PathBuf },
    HeldByOrphaned { pid: i32, lock_path: PathBuf },
    // file 存在但 flock 不在持有状态 — 理论不发生（flock 由 OS 在进程
    // 退出时自动释放）；万一遇到：(a) 调用方按 §4.1 决策树等同 HeldByLive
    // 处理（拒绝或 IPC），(b) diagnostic 输出多带一行 "stale lock file
    // detected, you may safely `rm <lock_path>` if no aleph process exists"。
}

pub fn try_acquire(data_dir: &Path) -> std::io::Result<AcquireOutcome>;

pub struct HolderDiagnostic {
    pub pid: i32,
    pub process_alive: bool,
    pub lock_path: PathBuf,
}

/// Read lock file metadata WITHOUT competing for the lock.
pub fn diagnose_holder(data_dir: &Path) -> Option<HolderDiagnostic>;
```

### 3.4 调用点改造

- **server `start`**：`try_acquire` 移到 `main` 入口最前段，**在 tracing init 之前**（tracing 不需要 lock 即可 boot；先拿锁再做任何事）。失败时 `eprintln!` 诊断 + `std::process::exit(64)`（EX_USAGE）。
- **CLI 写命令**（`secret set` / `memory write` / 等）：以 `try_acquire` 入手，按 §4 决策树分流。
- **`stop` / 诊断类**：用 `diagnose_holder` 不竞锁地读取信息。

### 3.5 Windows

当前 fallback 不锁。改为用 `fs2::FileExt::try_lock_exclusive`（fs2 在 Spec A 已是依赖），消除平台分叉。

### 3.6 测试

- **单元 `instance_lock_tests`**：tempdir 下 `try_acquire` × 3 → 第一次 Acquired，二三次 HeldByLive；drop 第一个之后第四次重新 Acquired。
- **集成 `tests/instance_lock_e2e.rs`**：fork 子进程持锁；父进程 `try_acquire` 得到 HeldByLive 且 pid 正确。
- **proptest**：`N ∈ [2,8]` 个 thread 并发 `try_acquire` → 恰好 1 个 Acquired，其余全 HeldByLive。

---

## 4. CLI 决策树 + IPC 转发协议 (G-c-1)

### 4.1 决策树

每个 CLI 子命令入口统一走：

```
CLI command starts
    │
    ▼
look up CommandPolicy { NoLock | LockOnly | LockOrIpc { route, method } }
    │
    ├─ NoLock  → execute locally（不打开 data_dir）
    │
    ├─ LockOnly → try_acquire(data_dir):
    │     ├─ Acquired                         → execute holding lock → drop on exit
    │     └─ HeldByLive | HeldByOrphaned      → exit 64
    │                     ("server is running on PID X. This operation requires
    │                       exclusive access. Run `aleph stop` first.")
    │
    └─ LockOrIpc → try_acquire:
          ├─ Acquired                         → execute locally holding lock
          └─ HeldByLive | HeldByOrphaned      → discover_endpoint() + forward via HTTP:
                ├─ endpoint missing      → exit 69
                │   ("server is initializing or crashed; try again or
                │     `aleph stop` first")
                ├─ token unreadable      → exit 73
                │   ("cannot read auth token from security.db read-only")
                ├─ 401 once              → re-read token + retry once
                ├─ 401 twice             → exit 77
                │   ("auth token rotated mid-call; retry")
                ├─ 5xx                   → forward server error message + exit 70
                └─ 2xx                   → format response + exit 0
```

Exit codes follow `sysexits.h` conventions (64 EX_USAGE, 69 EX_UNAVAILABLE, 70 EX_SOFTWARE, 73 EX_CANTCREAT, 77 EX_NOPERM).

### 4.2 CommandPolicy 注册表

```rust
pub enum HttpMethod { Get, Post, Delete }

pub enum CommandPolicy {
    NoLock,
    LockOnly,
    LockOrIpc { route: &'static str, method: HttpMethod },
}
```

预期分类（spec 实施时按现有 CLI 命令清点并标注）：

| 命令 | Policy | 说明 |
|---|---|---|
| `secret set/get/list/delete/import/export` | `LockOrIpc { /v1/admin/secrets/... }` | 高频，必须 IPC 不打扰 server |
| `memory write/clear/reset` | `LockOrIpc { /v1/admin/memory/... }` | 同上 |
| `agent create/delete/update` | `LockOrIpc { /v1/admin/agents/... }` | 同上 |
| `start` | 自带 lock 全生命周期 | 已有，仅迁早 |
| `stop` | NoLock + PID-file SIGTERM | 已有 |
| `status` / `--version` | NoLock | 不读 data_dir |
| 一次性 migration / dump / export-all | LockOnly | 必须独占 |

实施任务包括**全量审计** `src/bin/aleph-server/commands/` 每个子命令并标注 policy；漏标命令在 CI 阶段被反 regression 检查脚本（§7.4）拦截。

### 4.3 IPC 协议规约

#### 4.3.1 端点发现

server 启动绑定端口后**原子写**入：

```jsonc
// ~/.aleph/data/.ipc-endpoint.json (atomic temp+rename, chmod 600)
{
  "version": 1,
  "url": "http://127.0.0.1:8765",      // server 监听地址
  "pid": 12345,
  "started_at": "2026-05-02T01:00:00Z"
}
```

- 启动绑端口后写、graceful shutdown 时删；crash 留 stale → CLI 检测到 connection refused 时给 stale 提示 + `aleph stop --force` 引导。
- 写路径走 §6.1 `atomic_io::write_atomic`，文件权限 0600。

#### 4.3.2 认证

CLI 不走 OAuth/panel 登录，而是用同机已有 bearer：

- CLI 用 `open_sqlite_readonly(security_db_path)` (§6.1 helper) 打开 `~/.aleph/data/security.db`
- 调用一个新增的 free function `gateway::security::read_current_token_readonly(&conn) -> Result<Option<String>>`，内部 mirror 现有 `SharedTokenManager::current_token()` 的查询 SQL（`SELECT plaintext FROM shared_token ORDER BY id DESC LIMIT 1` 之类，落地时按当前实际 schema）— 把读路径从 `SharedTokenManager` 解耦出来，避免拖入全套写依赖。
- WAL 模式下读写互不阻塞 → 读到的 token 是某个一致 snapshot。
- 401 → 用同样路径重读 token 一次后重试（处理 rotation 在两个调用之间的窄窗口）。

#### 4.3.3 请求形态

标准 HTTP，`Authorization: Bearer <token>` + JSON body。响应也 JSON。无新协议、无新依赖（reqwest / serde_json / rusqlite 已在 deps）。

### 4.4 Server 侧

- 新增 `/v1/admin/*` namespace，挂在现有 axum gateway 上，handler 直接调用 server 内已有的 `SecurityStore` / `MemoryStore` / `AgentManager` 等 service 层。
- handler 是**薄壳**：deserialize JSON → 调 service → serialize 回 JSON。无业务逻辑。
- auth middleware 复用现有 `SharedTokenManager` 校验，权限模型最低要求"持有 admin bearer token"（CLI 同机用户能读 security.db 即视为受信）。
- 端点列表（落地时按 4.2 表格扩充）— **预计 ~10 个 handler，分 3 类**：
  - **secrets (4)**：`POST /v1/admin/secrets`、`GET /v1/admin/secrets`、`GET /v1/admin/secrets/:key`、`DELETE /v1/admin/secrets/:key`
  - **memory (3)**：`POST /v1/admin/memory/write`、`POST /v1/admin/memory/clear`、`POST /v1/admin/memory/reset`
  - **agents (3)**：`POST /v1/admin/agents`、`PATCH /v1/admin/agents/:id`、`DELETE /v1/admin/agents/:id`

### 4.5 静态封装

每个 LockOrIpc 命令需同时提供**本地实现**（拿到锁时执行）和**IPC 请求体**（server 跑时转发的 JSON）。helper 签名：

```rust
pub fn with_policy<L, T>(
    policy: CommandPolicy,
    local: L,
    ipc_body: serde_json::Value,        // 仅 LockOrIpc 用；NoLock/LockOnly 传 Value::Null
) -> anyhow::Result<T>
where
    L: FnOnce(&InstanceLock) -> anyhow::Result<T>,
    T: serde::de::DeserializeOwned + serde::Serialize,
```

分流：

- `NoLock`：忽略 lock 与 ipc_body，直接 `local(&dummy_lock)` — 但 `dummy_lock` 类型不存在，所以 NoLock 命令实际不该走 with_policy，而是另一个 `run_no_lock(f: FnOnce() -> Result<T>)` helper。
- `LockOnly`：`try_acquire` → Acquired 调 `local(&lock)`；HeldByLive/Orphaned exit 64。`ipc_body` 不使用。
- `LockOrIpc { route, method }`：`try_acquire` → Acquired 调 `local(&lock)`；HeldByLive/Orphaned 走 `forward_ipc(route, method, ipc_body)` → 反序列化为 `T` 返回。

所有 CLI 子命令在 `main` 里强制经 `with_policy` 或 `run_no_lock` 跑，避免新命令漏掉决策树。反 regression（§7.4）扫描 `src/bin/aleph-server/commands/` 强制每个命令文件含 `with_policy(` 或 `run_no_lock(` 字样。

### 4.6 测试

- **单元**：每条 policy 分支 mock test。
- **集成 `tests/cli_ipc_e2e.rs`**：spawn 真实 server → spawn `aleph secret set` 子进程 → 断言走了 IPC 路径（server log 看到 admin endpoint 命中）+ secret 实际写入。
- **集成 `tests/cli_no_server_e2e.rs`**：server 不跑 → spawn `aleph secret set` → 断言走了本地 lock 路径 + secret 实际写入。
- **集成 `tests/cli_locked_no_ipc_e2e.rs`**：spawn 一个**只持锁不开 IPC 端口**的 mock 进程（手动占 lock 文件） → spawn `aleph secret set` → 断言得到 endpoint missing 明确提示 + exit 69。
- **token rotation race**：mock server 在第一次 401 后允许第二次成功 → CLI 应自愈不报错。

---

## 5. Vault + acp_sessions 文件加固 (G-c-3)

### 5.1 范围

| 文件 | 当前状态 | 加固措施 |
|---|---|---|
| `secrets.vault` | 直接 `fs::write` / `fs::read` | atomic temp+rename **+** 邻接 `.lock` 文件 fcntl |
| `acp_sessions.json` | 直接 `fs::write` | atomic temp+rename，**无锁** |
| `vault.db` (SQLite) | 直连 | 走 §6 的 WAL+busy_timeout，**不**叠 fcntl |

### 5.2 共享原语

抽 `src/utils/atomic_io.rs`（新建文件，预计 ~80 行）：

```rust
/// Write bytes to `path` atomically: write to `<path>.tmp.<rand>`, fsync, rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()>;

/// Acquire fs2 exclusive advisory lock on `<path>.lock`, run `f`, release on drop.
/// `f` receives the locked guard so it cannot be called outside the critical section.
pub fn with_file_lock<T, F: FnOnce(&FileLockGuard) -> std::io::Result<T>>(
    lock_path: &Path,
    f: F,
) -> std::io::Result<T>;

pub struct FileLockGuard { /* RAII; drop unlocks */ }
```

> Spec A 在 `src/memory/curated/format.rs` 内联了类似逻辑但**不导出**，作用域局限。这里抽 `alephcore::utils::atomic_io` 是为 Spec C 自身和未来复用，**不动 Spec A 内部** — 避免范围漂移。后续如想 dedupe，单独发 cleanup PR。

### 5.3 调用点改造

#### 5.3.1 `secrets.vault`

定位：grep 出所有 `secrets.vault` 直读直写 → 包一层 `vault_io`（新文件，预计 ~50 行）：

```rust
pub struct VaultIo {
    path: PathBuf,
    lock_path: PathBuf,
}

impl VaultIo {
    pub fn new(data_dir: &Path) -> Self;
    pub fn read(&self) -> std::io::Result<Vec<u8>>;             // with_file_lock + read
    pub fn write(&self, bytes: &[u8]) -> std::io::Result<()>;   // with_file_lock + write_atomic
}
```

所有 vault 文件 IO 必须走 `VaultIo`，禁止散落直读 — 实施时加 grep 检查作为验收门。

#### 5.3.2 `acp_sessions.json`

`src/acp/manager.rs:24` 的 `acp_sessions_path()` 已锁定唯一写路径。改造：

- save 路径：`fs::write` → `atomic_io::write_atomic`
- 读路径：不变（裸读 OK，atomic write 保证读到的永远是完整文件）

不加 fcntl，原因：singleton 已物理保护单写者，acp_sessions 本身可重建（丢失 = 启动新会话），加锁是 overkill。

### 5.4 测试

#### Vault

- **`tests/vault_atomic_e2e.rs`**：写 100 KB → SIGKILL 模拟（在写中段 panic）→ 断言文件要么完整旧版要么完整新版，**绝无半写状态**。
- **`tests/vault_concurrent_e2e.rs`**：两 thread 并发 `VaultIo::write` 不同 keys → fcntl 串行化 → 最终文件含两次写入合并、不丢、不撕格式。
- **proptest**：`N ∈ [2,8]` 个 thread 各写 1 KB → 全部串行成功、文件长度 = N×1024。

#### acp_sessions

- **`tests/acp_atomic_e2e.rs`**：写中段 SIGKILL → 文件要么旧 JSON 要么新 JSON，绝无半写。

#### 反 regression grep

实施验收最后一步：

```bash
git grep -n 'secrets\.vault\|acp_sessions\.json' src/ \
  | grep -v 'vault_io\|atomic_io\|acp/manager.rs:24\|test'
```

必须无输出，否则视为有人绕过新封装。

---

## 6. SQLite 统一加固 (G-c-2)

### 6.1 共享 helper

新建 `src/utils/sqlite_open.rs`（预计 ~40 行）：

```rust
/// Open a SQLite connection on `path` with cross-process safety pragmas:
/// - journal_mode=WAL    (concurrent reads + writer-friendly)
/// - busy_timeout=5000   (5s wait on lock contention before SQLITE_BUSY)
/// - synchronous=NORMAL  (WAL-safe, faster than FULL)
/// - foreign_keys=ON     (existing project convention)
pub fn open_sqlite_safe(path: &Path) -> rusqlite::Result<rusqlite::Connection>;

/// Same but with `OpenFlags::SQLITE_OPEN_READ_ONLY` for CLI / IPC token-read paths.
pub fn open_sqlite_readonly(path: &Path) -> rusqlite::Result<rusqlite::Connection>;
```

### 6.2 迁移面

grep 当前所有 `Connection::open(`、`rusqlite::Connection::open(` 等位点（预计 15-20 处），分两类处理：

1. **已设置 WAL+busy_timeout 的（4 处）** — `memory.db` / `cron.db` / `heartbeat.db` / `tasks/shared` → 替换为调用 helper，删掉内联 PRAGMA。
2. **没有 WAL 的剩余 DB（~12 处）** — `state.db` / `notes.db` / `sessions.db` / `channels.db` / `coord.db` / `coord_tasks.db` / `teams.db` / `vault.db` / `security.db` / `agent_env.db` / `agent_envs.db` / `pairing.db` / `devices.db` / `workspaces.db` → 替换为调用 helper。

### 6.3 反 regression

实施验收最后一步：

```bash
git grep -n "Connection::open(\|rusqlite::Connection::open(" src/ --include='*.rs' \
  | grep -v "open_sqlite_safe\|open_sqlite_readonly\|test"
```

必须无输出。

### 6.4 测试

- **单元 `sqlite_open_tests`**：tempdir → `open_sqlite_safe` → 查询 `PRAGMA journal_mode` 应返回 `wal`、`PRAGMA busy_timeout` 应返回 `5000`、`PRAGMA synchronous` 应返回 `1` (NORMAL)。
- **集成 `tests/sqlite_concurrent_read_e2e.rs`**：1 个 writer thread 持续插数据 + 4 个 reader thread 并发查询 → 不出现 `SQLITE_BUSY` panic、reader 永远看到一致 snapshot。

### 6.5 不做（YAGNI 复述）

- 不加 migration 锁 — singleton + IPC 已保单写者
- 不加 retry/backoff — busy_timeout=5000 已给 5s 等待窗口
- 不动 SQLite cipher / encryption 配置 — 与 Spec C 范围正交

---

## 7. 集成测试 + 验收

### 7.1 端到端集成测试矩阵

| Scenario | 测试文件 | 期望 |
|---|---|---|
| 双 server 并发启动 | `tests/spec_c_double_start.rs` | 第 1 个 spawned 子进程拿锁；第 2 个 spawn 在 50ms 内 `exit 64`，stderr 含 PID + "vault corruption" 警告 |
| Server up + CLI write via IPC | `tests/spec_c_cli_ipc.rs` | spawn server、spawn `aleph secret set k=v` → 走 `/v1/admin/secrets` endpoint（mock 计数 +1）→ 退出 0、`secret get` 读出 v |
| Server up + CLI write 被拒（无 IPC route） | `tests/spec_c_cli_refuse.rs` | spawn `aleph migrate-foo`（LockOnly policy）→ 退出 64、stderr 提示 `aleph stop` |
| Server up + CLI 401 自愈 | `tests/spec_c_cli_token_rotation.rs` | mock server 第一次 401 → CLI 重读 token 重试 → 第二次 200 → 退出 0 |
| Server up + endpoint file 缺失 | `tests/spec_c_cli_endpoint_missing.rs` | mock 进程持锁但不写 `.ipc-endpoint.json` → CLI 退出 69、stderr 提示 stale endpoint |
| Server down + CLI write via local lock | `tests/spec_c_cli_no_server.rs` | server 不跑 → `aleph secret set` 走本地 lock 路径 → 退出 0 |
| Vault crash-safe write | `tests/vault_atomic_e2e.rs` | 写中段 panic → 文件完整旧版 |
| Vault concurrent write | `tests/vault_concurrent_e2e.rs` | 2 thread 并发 → fcntl 串行化、不丢内容 |
| acp_sessions crash-safe write | `tests/acp_atomic_e2e.rs` | 写中段 SIGKILL → 文件完整旧版或完整新版 |
| SQLite concurrent read 不阻塞 | `tests/sqlite_concurrent_read_e2e.rs` | 1 writer + 4 reader 并发，无 BUSY panic |

### 7.2 验收门 (acceptance criteria)

1. `target/release/aleph-server start` 启动后再 `target/release/aleph-server start` → 第二个 50ms 内退出，错误信息含活进程 PID + 引导 `aleph stop`
2. Server 跑着时 `aleph secret set foo=bar` → 退出 0、走 IPC（server log 命中 `/v1/admin/secrets`），`aleph secret get foo` → bar
3. Server 不跑时 `aleph secret set foo=bar` → 退出 0、走本地 lock 路径，再 `aleph secret get foo` → bar
4. SIGKILL server → 立即 `aleph-server start`（不等 sleep 2 秒）→ 启动成功，无 vault corruption（CLAUDE.md 那条警告本身被这条验收消解掉）
5. 同时启动 8 个 `aleph` 子命令并发 → 恰好 1 个能拿锁、其余正确分流（IPC 或拒绝）、无任何 panic
6. `cargo test --lib` 全绿；新增的 `cargo test --test 'spec_c_*' --test 'instance_lock_e2e' --test 'cli_*_e2e' --test 'vault_*_e2e' --test 'acp_atomic_e2e' --test 'sqlite_concurrent_read_e2e'` 全绿
7. `cargo clippy -- -D warnings` 干净
8. `git grep` 反 regression 检查（vault 直读、`Connection::open` 不走 helper、`secrets.vault` / `acp_sessions.json` 散落）全部空输出
9. CLAUDE.md 进程管理章节更新：删掉"等 2 秒让文件锁释放"等过时建议，描述 Spec C 后的实际语义
10. **手动冒烟**：实际跑下列序列无 vault corruption / 无数据丢失：
    - 起 server → `aleph secret set` → 验证生效
    - 起 server → 起 `aleph secret set` → 走 IPC，secret 写入
    - 起 server → SIGKILL → 立即起新 server → 起 `aleph secret list`，旧 secrets 完整
    - 起 8 个 CLI 并发，全部 exit 0 或 64/69 with 明确诊断

### 7.3 任务规模估算

| 模块 | 预估 task 数 |
|---|---|
| Discovery / API audit (Spec B 模式) — 第 0 任务 | 1 |
| §3 instance_lock 抽核心 + Drop guard + 结构化 enum + Windows fs2 | 3 |
| §4 CLI 决策树 + CommandPolicy 注册表 + with_policy helper + 全 CLI 命令审计 | 4 |
| §4 IPC 端点发现 + atomic write `.ipc-endpoint.json` + 启动/退出钩子 | 2 |
| §4 server `/v1/admin/*` namespace + auth middleware 复用 + ≈10 handler (secrets/memory/agents) | 4 |
| §4 CLI HTTP client + token read-only + 401 retry | 2 |
| §5 atomic_io.rs + with_file_lock + 单测 | 1 |
| §5 VaultIo 包装 + 调用点改造 + crash-safe 测试 | 2 |
| §5 acp_sessions.json atomic write 改造 | 1 |
| §6 sqlite_open_safe / readonly helper + 迁移所有 callers | 2 |
| §7 10 个端到端集成测试 | 4 |
| §7 反 regression grep 验收脚本 + CLAUDE.md 文档更新 | 1 |
| Final acceptance review | 1 |
| **总计** | **~28** |

参考 Spec A 26 task / Spec B 21 task，**Spec C ~28 task** 在合理体量上限。

### 7.4 反 regression 检查脚本

实施时维护 `scripts/spec_c_regression.sh`（在 acceptance review 任务里执行）：

```bash
#!/usr/bin/env bash
set -euo pipefail

# 1. SQLite 必须走 helper
if git grep -n "Connection::open(\|rusqlite::Connection::open(" src/ --include='*.rs' \
   | grep -v "utils/sqlite_open\.rs\|test"; then
  echo "❌ direct rusqlite open detected — must use open_sqlite_safe"
  exit 1
fi

# 2. Vault / acp_sessions 必须走封装
if git grep -n 'secrets\.vault\|acp_sessions\.json' src/ \
   | grep -v 'vault_io\|atomic_io\|acp/manager.rs\|test'; then
  echo "❌ raw access to secrets.vault or acp_sessions.json detected"
  exit 1
fi

# 3. CLI 命令必须有 policy 标注
if git grep -L "CommandPolicy::" src/bin/aleph-server/commands/ \
   | grep '\.rs$' | grep -v 'mod\.rs'; then
  echo "❌ CLI command without CommandPolicy annotation"
  exit 1
fi

echo "✅ Spec C regression checks pass"
```

---

## 8. Migration / Rollout

### 8.1 Backwards Compatibility

- 现有 `aleph-server start` 行为：先取锁、再 init — 与 Spec C 后只差**取锁时机更早**，对现有用户透明。
- 现有 CLI 命令在用户没并发使用时行为不变；并发时从 corruption 改为 IPC 转发（更好）或 exit 64 拒绝（明确）。
- `daemon::acquire_instance_lock` 保留薄 re-export，兼容旧调用方一次实施周期，下个 spec 删除。

### 8.2 Phasing

Spec C 一次性发版（参考 Spec A/B 模式），不做灰度 — 所有改动都是**收紧**安全网，无新业务 surface。

### 8.3 Cleanup

实施完成后删除：

- `src/bin/aleph-server/daemon.rs` 内 `acquire_instance_lock` 实现体（保留薄 re-export）
- 散落的 `journal_mode=WAL; PRAGMA busy_timeout=5000` 内联（迁移到 `open_sqlite_safe`）
- 散落的 `secrets.vault` / `acp_sessions.json` 直读直写（迁移到 `VaultIo` / `atomic_io::write_atomic`）

---

## 9. 风险 + 缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| `.ipc-endpoint.json` stale（server crash 后留残） | CLI 走 IPC 收 connection refused | 错误信息明确指引 `aleph stop --force` + 重启自动覆盖 |
| WAL 在 SQLite 跨某些奇怪文件系统不可用 | 启动失败 | data_dir 仅本地约束，README/CLAUDE.md 已规定 |
| Bearer token rotation race | CLI 401 一次 | API 401 自愈重试 |
| Windows `fs2::try_lock_exclusive` 在某些网络盘失败 | 单进程锁失效 | data_dir 必须本地（同 SQLite 约束）+ 错误诊断 |
| `/v1/admin/*` endpoint 被远端攻击者利用 | 越权写 vault | 仅监听 127.0.0.1（已是默认）+ bearer 校验 |
| CLI 并发 8+ 实例时 token 频繁 rotate | 多个 401 retry | bearer 在 server 生命周期通常稳定，rotate 是稀有事件 |

---

## 10. References

- Spec A 设计：`docs/superpowers/specs/2026-05-01-memory-evolution-spec-a-curated-hot-snapshot-design.md`
- Spec B 设计：`docs/superpowers/specs/2026-05-01-memory-evolution-spec-b-session-search-summarization-design.md`
- 4-Spec roadmap：`docs/superpowers/specs/2026-04-13-memory-evolution-roadmap.md`
- 现有 singleton 实现：`src/bin/aleph-server/daemon.rs:53-126`
- 现有 SharedTokenManager：`src/gateway/security/shared_token.rs`
- 现有 acp_sessions：`src/acp/manager.rs:24-46`
- CLAUDE.md 进程管理章节（待更新）：根目录 CLAUDE.md
