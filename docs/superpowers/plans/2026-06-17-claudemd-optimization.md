# CLAUDE.md 优化实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把根 `CLAUDE.md` 从 350 行瘦身到 210–250 行（保宪法 R1-R10/P1-P8 逐字不动），周边操作性详解外迁到 `docs/reference/`，并补齐文章经验 #2（Do NOT introduce）、#5（子目录 CLAUDE.md）、#8（Working Style）。

**Architecture:** 纯文档重构。每个外迁任务遵循"先确保内容落在目标文档 → 再把 CLAUDE.md 对应块替换为一行指针 → 验证信息无丢失 → 提交"。新增内容直接插入根 CLAUDE.md。两个子目录 CLAUDE.md 从根文档既有红线提炼，不新造约束。

**Tech Stack:** Markdown only. 验证用 `wc -l` / `grep` / `git diff`，不涉及 cargo / 构建。

## Global Constraints

- **R1-R10（CLAUDE.md 行 3-86）与 P1-P8（行 88-146）逐字不改**——任何任务结束时 `git diff` 这两段必须为空。
- **绝不丢失信息**：外迁块必须先在目标文档中可查到，再从 CLAUDE.md 删除。
- **所有被 CLAUDE.md 引用的外迁文档一律放在 `docs/reference/` 下**（用户硬约束）。
- 子目录 CLAUDE.md（`src/harness/`、`src/gateway/`）就近放源码目录，不归入 `docs/reference/`。
- **不执行任何代码修复**；不新建 `.claude/hooks/`；不在项目内新建 MEMORY.md。
- 提交信息英文，格式 `<scope>: <description>`，全局禁用 attribution（不加 Co-Authored-By）。
- 只提交 `CLAUDE.md`、`docs/reference/*`、`src/*/CLAUDE.md`；`docs/superpowers/` 被 gitignore，不提交。
- 根 CLAUDE.md 最终目标 210–250 行；不强求 200（保宪法权衡）。

---

### Task 1: 新建 docs/reference/RELEASE.md 并把发版流程详解外迁

**Files:**
- Create: `docs/reference/RELEASE.md`
- Modify: `CLAUDE.md:224-235`（发版流程整段）

**Interfaces:**
- Produces: 文档索引将引用 `docs/reference/RELEASE.md`（Task 10 消费）。

- [ ] **Step 1: 创建 `docs/reference/RELEASE.md`**

内容（从 CLAUDE.md 行 224-235 原样搬运并补标题）：

```markdown
# 发版流程 (Release Process)

> 由根 `CLAUDE.md` 的「发版流程」一行指针指向本文。

**由 AI (Claude) 驱动的两步流程：**

1. **AI 写版本日志** — 读取**上一个 release 版本到 HEAD 之间**的 git log（通过 `git log <上次release commit>..HEAD`），总结 10-20 条有价值的内容，分为 Added（新增功能）和 Fixed（修复）两个分类，写入 CHANGELOG.md
2. **运行 `just release YY.M.D`** — 自动完成：版本号更新 + 提交推送 + 触发**三产物 × 三平台**构建并发布（完整桌面 App 内置 `aleph-server` / Aleph Panel 纯壳 App / 独立 `aleph-server` 二进制 + `install.sh`，同一 GitHub Release）

`just release` 会校验 CHANGELOG.md 中是否有对应版本的条目，没有则拒绝发布。GitHub Release 页面自动从 CHANGELOG.md 提取版本日志。

> **可选预检**：发布前可先跑 `just verify-build`，以 build-only 模式在 CI 上构建**三产物 × 三平台**（完整 App / Panel 纯壳 App / 独立 server，只构建 + 传 artifacts，不打 tag、不发布），确认都能正常构建后再 `just release`。同一 workflow（`aleph-app-release.yml`）的 `publish` 输入：`off`=纯验证，`on`=`just release` 走的发布模式。

> **监控 CI（每次发版复用）**：用 `python3 scripts/poll_release_run.py`（不带参数自动选最新 run）做 **fail-fast 轮询**——job 级检查，任一平台 job 失败/取消立即退出并打印是哪个平台，而不是像 `gh run watch --exit-status` 那样整轮（~33 分钟）才返回、漏报单平台早败。脚本内部循环、守 gh 空响应/超时（开发网络有过 DNS/代理瞬断），budget（默认 540s）用尽后打印 `RESULT=STILL_RUNNING`，直接再跑一次即可。最后一行 `RESULT=COMPLETED conclusion=success` 全绿 / `RESULT=FAILED` 附失败 job 名。**平台门控陷阱**：CI 失败常是 `#[cfg(target_os)]` 分支里的 `const fn` 调运行时函数——本地 macOS 不编译该分支故看不见，失败后一次性 grep `desktop/{linux,windows,shell,shared}` 全部 const fn 亲读每个 body。
```

- [ ] **Step 2: 把 CLAUDE.md 行 224-235 整段替换为指针**

将「### 发版流程 (Release Process)」标题下的全部内容（行 226-235）替换为：

```markdown
### 发版流程 (Release Process)

`just release YY.M.D` 触发三产物×三平台构建发布（发版前先写 CHANGELOG.md）。完整两步流程、`just verify-build` 预检、CI fail-fast 轮询（`scripts/poll_release_run.py`）详见 [RELEASE.md](docs/reference/RELEASE.md)。
```

- [ ] **Step 3: 验证信息无丢失**

Run: `grep -c "poll_release_run.py" docs/reference/RELEASE.md`
Expected: `1`（CI 轮询细节已落到 RELEASE.md）

Run: `grep -c "just verify-build" docs/reference/RELEASE.md`
Expected: `1`

- [ ] **Step 4: 验证宪法未动**

Run: `git diff CLAUDE.md | grep -E '^\-' | grep -E 'R1[0-9]?\.|P[1-8]\.'`
Expected: 无输出（红线/原则行未被删除）

- [ ] **Step 5: 提交**

```bash
git add CLAUDE.md docs/reference/RELEASE.md
git commit -m "docs: externalize release process to RELEASE.md"
```

---

### Task 2: 新建 docs/reference/PROCESS_MANAGEMENT.md 并把进程管理详解外迁

**Files:**
- Create: `docs/reference/PROCESS_MANAGEMENT.md`
- Modify: `CLAUDE.md:259-277`（进程管理整段）

**Interfaces:**
- Produces: 文档索引将引用 `docs/reference/PROCESS_MANAGEMENT.md`（Task 10 消费）。

- [ ] **Step 1: 创建 `docs/reference/PROCESS_MANAGEMENT.md`**

内容（从 CLAUDE.md 行 259-277 原样搬运并补标题）：

```markdown
# 进程管理 (Process Management)

> 由根 `CLAUDE.md` 的「进程管理」一行指针指向本文。

Singleton 强制由 OS 级 `flock` 保证（Spec C, 2026-05-02 起改为结构化保护）：

- `aleph-server start` 在 `main()` 进入任何 DB/vault 操作之前先获取
  `~/.aleph/data/aleph.lock`。第二个 `start` 会立即以 exit 64 退出，
  并在 stderr 打印持锁进程的 PID。
- 所有 CLI 写子命令（`secret`、`hooks`、`plugins` 等）通过
  `with_policy` 分发：服务在跑时，写操作通过 `/v1/admin/*` IPC 转发；
  服务不在时，CLI 自己拿锁本地写入。两条路径都不会与服务竞争。
- OS 在进程退出（正常、panic、SIGKILL）时自动释放 `flock`。`kill -9 <pid>`
  之后**无需 sleep**，可立即 `aleph-server start`。
- 反向回归脚本 `scripts/spec_c_regression.sh` 锁住四条不变量：
  SQLite 走 `open_sqlite_safe`、vault/acp 走 `vault_io`/`atomic_io`、
  每个 CLI 子命令显式声明 policy、`acquire_instance_lock` 不再有遗留 caller。

如果看到 `Stale lock file detected (PID X not running)`，可以安全
`rm ~/.aleph/data/aleph.lock`（理论上不会出现，因为 flock 是 OS 管理的；
该诊断仅作防御性提示）。
```

- [ ] **Step 2: 把 CLAUDE.md 行 259-277 整段替换为指针**

将「### 进程管理 (Process Management)」标题下的全部内容（行 261-277）替换为：

```markdown
### 进程管理 (Process Management)

Singleton 由 OS 级 `flock`（`~/.aleph/data/aleph.lock`）强制；CLI 写子命令经 `with_policy` 走 IPC 或本地拿锁，不与服务竞争。`kill -9` 后可立即重启。Spec C 不变量与回归脚本详见 [PROCESS_MANAGEMENT.md](docs/reference/PROCESS_MANAGEMENT.md)。
```

- [ ] **Step 3: 验证信息无丢失**

Run: `grep -c "spec_c_regression.sh" docs/reference/PROCESS_MANAGEMENT.md`
Expected: `1`

Run: `grep -c "open_sqlite_safe" docs/reference/PROCESS_MANAGEMENT.md`
Expected: `1`

- [ ] **Step 4: 提交**

```bash
git add CLAUDE.md docs/reference/PROCESS_MANAGEMENT.md
git commit -m "docs: externalize process management to PROCESS_MANAGEMENT.md"
```

---

### Task 3: 把 Windows 构建详解外迁到 WINDOWS_RUNTIME.md

**Files:**
- Modify: `docs/reference/WINDOWS_RUNTIME.md`（追加构建小节，若未覆盖）
- Modify: `CLAUDE.md:187-214`（Windows 构建整段）

**Interfaces:**
- Consumes: 既有 `docs/reference/WINDOWS_RUNTIME.md`（已存在，文档索引行 324 已引用）。

- [ ] **Step 1: 核对目标文档是否已覆盖 Windows 构建前置依赖表**

Run: `grep -c "wasm-bindgen-cli" docs/reference/WINDOWS_RUNTIME.md`
Expected: 若返回 `0`，说明依赖表未覆盖，执行 Step 2；若 `>=1`，跳过 Step 2 直接到 Step 3。

- [ ] **Step 2: （仅当 Step 1 返回 0）把 CLAUDE.md 行 187-214 的构建内容追加到 WINDOWS_RUNTIME.md**

在 `docs/reference/WINDOWS_RUNTIME.md` 末尾追加一节 `## Windows 构建`，原样搬运 CLAUDE.md 行 189-214 的内容（justfile 跨平台说明、一次性前置依赖表、全量构建+启动 PowerShell 块、dev 模式说明）。

- [ ] **Step 3: 把 CLAUDE.md 行 187-214 整段替换为指针**

将「### Windows 构建」标题下全部内容替换为：

```markdown
### Windows 构建

`just shell-build` / `just shell-dev` 在 Windows 同样适用（justfile 已守卫 macOS 专属步骤、自动追加 `.exe`），产物为 NSIS `.exe` + `.msi`。一次性前置依赖（MSVC / WebView2 / protoc / wasm 目标 / `wasm-bindgen-cli` 版本对齐 / `cargo-tauri` / Git for Windows `usr\bin` 入 PATH）与全量构建步骤详见 [WINDOWS_RUNTIME.md](docs/reference/WINDOWS_RUNTIME.md)。
```

- [ ] **Step 4: 验证信息无丢失**

Run: `grep -c "wasm-bindgen-cli" docs/reference/WINDOWS_RUNTIME.md`
Expected: `>=1`（依赖版本对齐这条关键陷阱在目标文档可查）

- [ ] **Step 5: 提交**

```bash
git add CLAUDE.md docs/reference/WINDOWS_RUNTIME.md
git commit -m "docs: externalize Windows build steps to WINDOWS_RUNTIME.md"
```

---

### Task 4: 把 Panel↔Daemon 资源嵌入链外迁到 DESKTOP_SHELL.md

**Files:**
- Modify: `docs/reference/DESKTOP_SHELL.md`（追加嵌入链小节，若未覆盖）
- Modify: `CLAUDE.md:176-185`（Panel 嵌入链 blockquote）

**Interfaces:**
- Consumes: 既有 `docs/reference/DESKTOP_SHELL.md`（已存在，文档索引行 323 已引用）。

- [ ] **Step 1: 核对目标文档是否已覆盖刷新链**

Run: `grep -c "rust_embed" docs/reference/DESKTOP_SHELL.md`
Expected: 返回 `0` → 执行 Step 2；`>=1` → 跳过 Step 2。

- [ ] **Step 2: （仅当 Step 1 返回 0）把嵌入链内容追加到 DESKTOP_SHELL.md**

在 `docs/reference/DESKTOP_SHELL.md` 末尾追加一节 `## Panel ↔ Daemon 资源嵌入链与刷新`，原样搬运 CLAUDE.md 行 176-185 的内容（rust_embed 编译期嵌入、三步刷新链、dev / .app / Windows 三种 daemon 替换法、"单跑 just wasm 不够"的说明）。

- [ ] **Step 3: 把 CLAUDE.md 行 176-185 整段替换为指针**

将该 blockquote 整段替换为：

```markdown
> **⚠️ Panel ↔ Daemon 资源嵌入链**: Panel UI 经 `rust_embed` 在 `aleph-server` **编译时**静态嵌入二进制，运行中的 daemon 不读磁盘 dist/*。改完 panel 看不到效果＝漏了重编 binary。完整刷新链（`just wasm` → 重编 server → 替换运行中 binary，dev / macOS .app / Windows 三种 daemon 替换法）详见 [DESKTOP_SHELL.md](docs/reference/DESKTOP_SHELL.md)。
```

- [ ] **Step 4: 验证信息无丢失**

Run: `grep -c "rust_embed" docs/reference/DESKTOP_SHELL.md`
Expected: `>=1`

- [ ] **Step 5: 提交**

```bash
git add CLAUDE.md docs/reference/DESKTOP_SHELL.md
git commit -m "docs: externalize panel embed-refresh chain to DESKTOP_SHELL.md"
```

---

### Task 5: 精简分发形态与信任模型两段为指针

**Files:**
- Modify: `docs/reference/PRODUCT_TOPOLOGY.md`（核对覆盖，若未覆盖则补）
- Modify: `CLAUDE.md:172-174`（分发形态 + 信任模型 blockquote 组）

**Interfaces:**
- Consumes: 既有 `docs/reference/PRODUCT_TOPOLOGY.md`（行 293 已引用）、`docs/reference/SECURITY.md`（行 311/284 已引用）。

- [ ] **Step 1: 核对 PRODUCT_TOPOLOGY.md 是否覆盖三产物形态**

Run: `grep -c "externalBin" docs/reference/PRODUCT_TOPOLOGY.md`
Expected: 返回 `0` → 在 PRODUCT_TOPOLOGY.md 末尾追加一节 `## 三产物分发形态`，搬运 CLAUDE.md 行 172 分发形态内容；`>=1` → 跳过补写。

- [ ] **Step 2: 把 CLAUDE.md 行 172-174 替换为精简指针**

将「分发形态」+「信任模型 = 网络边界」两个 blockquote（行 172-174）替换为：

```markdown
> **分发形态**: Aleph 同一 tag 发三产物——完整桌面 App（内置 `aleph-server`，单机零配置）、Aleph Panel 纯壳 App（连局域网 server）、独立 `aleph-server` 二进制（`install.sh` / `install.ps1`）。详见 [PRODUCT_TOPOLOGY.md](docs/reference/PRODUCT_TOPOLOGY.md)。
>
> **信任模型 = 网络边界**: 默认只绑 `127.0.0.1`；`[gateway] host = "0.0.0.0"` 显式开放局域网。方法级门槛是 device tier（远程 Panel 默认 Chat tier，config 类 RPC 须 operator 提权），协议护栏是 WS Origin 校验。详见 [SECURITY.md#auth-ux](docs/reference/SECURITY.md#auth-ux)。
```

- [ ] **Step 3: 验证信任模型关键词仍在 SECURITY.md（已有引用，仅确认）**

Run: `grep -c "device tier\|method_authz\|Origin" docs/reference/SECURITY.md`
Expected: `>=1`（信任模型细节本就在 SECURITY.md；若为 0，把 CLAUDE.md 原行 174 的 device tier / origin_policy 细节补进 SECURITY.md 再继续）

- [ ] **Step 4: 提交**

```bash
git add CLAUDE.md docs/reference/PRODUCT_TOPOLOGY.md docs/reference/SECURITY.md
git commit -m "docs: slim distribution and trust-model notes to pointers"
```

---

### Task 6: 把 Git Worktree 注意事项外迁到 CODE_ORGANIZATION.md

**Files:**
- Modify: `docs/reference/CODE_ORGANIZATION.md`（追加小节）
- Modify: `CLAUDE.md:255-257`（Git Worktree 注意事项）

**Interfaces:**
- Consumes: 既有 `docs/reference/CODE_ORGANIZATION.md`（行 313 已引用）。

- [ ] **Step 1: 把内容追加到 CODE_ORGANIZATION.md**

在 `docs/reference/CODE_ORGANIZATION.md` 末尾追加：

```markdown
## Git Worktree 注意事项

`EnterWorktree` 会在每次 Bash 命令后强制重置 CWD 到 worktree 目录，即使 `cd` 切回主仓库也无效。因此在同一会话内执行 `git worktree remove` 会导致 Shell 永久损坏。**正确做法**：在 `EnterWorktree` 会话内只合并不删除，用新会话清理 worktree；或不用 `EnterWorktree`，手动用绝对路径管理。
```

- [ ] **Step 2: 从 CLAUDE.md 删除「### Git Worktree 注意事项」整段（行 255-257）并加一行指针**

将行 255-257 替换为：

```markdown
### Git Worktree 注意事项

`EnterWorktree` 会话内只合并不删除（同会话 `git worktree remove` 会损坏 Shell）。详见 [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md)。
```

- [ ] **Step 3: 验证信息无丢失**

Run: `grep -c "EnterWorktree" docs/reference/CODE_ORGANIZATION.md`
Expected: `>=1`

- [ ] **Step 4: 提交**

```bash
git add CLAUDE.md docs/reference/CODE_ORGANIZATION.md
git commit -m "docs: externalize git worktree caveat to CODE_ORGANIZATION.md"
```

---

### Task 7: 新增「Tech Stack & Do NOT introduce」区块（经验 #2）

**Files:**
- Modify: `CLAUDE.md`（在 P8 段后、`## 🔧 开发指南` 前插入新区块，即行 146 与 148 之间）

**Interfaces:**
- Produces: 无下游消费（独立区块）。

- [ ] **Step 1: 用 grep 核对锁定项属实（不调 cargo）**

Run: `grep -E "^name = \"(async-std|smol|tokio)\"" Cargo.lock | sort -u`
Expected: 看到 `tokio`，**不应**看到 `async-std`/`smol`（确认 tokio 是唯一 async runtime）。若出现 async-std/smol，从禁用清单删掉该条。

Run: `grep -E "^name = \"(sqlite-vec|rusqlite|libsqlite3-sys)\"" Cargo.lock | sort -u`
Expected: 看到 sqlite 相关条目（确认记忆层锁 sqlite + sqlite-vec）。

- [ ] **Step 2: 在 CLAUDE.md P8 段后插入新区块**

在 `### P8. LLM 优先` 段结束的 `---`（行 146）之后、`## 🔧 开发指南`（行 148）之前插入：

```markdown
## 🛠 技术栈与禁用清单 (Tech Stack & Do NOT introduce)

**核心栈**: Rust Core (tokio + serde) · 记忆层 SQLite + sqlite-vec · 接口 JSON Schema (schemars) · Panel Leptos/WASM · 桌面壳 Tauri。

**Do NOT introduce unless explicitly requested**（基于 R1/R3/R7 推导，违者不得合入）:

- **第二个 async runtime**（async-std / smol）—— 全栈锁定 tokio
- **独立向量数据库 client 进 core**（qdrant / lancedb / milvus 等）—— 记忆层已锁 sqlite + sqlite-vec
- **`src` 中直接依赖平台 API crate**（windows-rs / core-graphics / cocoa / objc / winapi）—— 违 R1，必须走原生 Bridge IPC
- **正则 / 规则引擎做意图识别或路由**—— 违 R7/P8，语义判断交 LLM
- **非 serde 的序列化栈**—— 全栈 serde

---
```

- [ ] **Step 3: 验证区块就位且红线未动**

Run: `grep -c "Do NOT introduce" CLAUDE.md`
Expected: `1`

Run: `git diff CLAUDE.md | grep -E '^\-' | grep -E '### R[0-9]|### P[0-9]'`
Expected: 无输出（红线/原则标题行未被删）

- [ ] **Step 4: 提交**

```bash
git add CLAUDE.md
git commit -m "docs: add tech stack and do-not-introduce denylist"
```

---

### Task 8: 新增/增强「My Working Style」并折叠 Session Context（经验 #8）

**Files:**
- Modify: `CLAUDE.md:249-253`（语言规范）、`CLAUDE.md:342-350`（Session Context）

**Interfaces:**
- Produces: 无下游消费。

- [ ] **Step 1: 把「### 语言规范」（行 249-253）扩展为「My Working Style」**

将行 249-253 的「### 语言规范」整段替换为：

```markdown
### My Working Style

- 先给方案再写代码；不确定时列出选项，不猜测（呼应 P1 与全局 CLAUDE.md）
- 重大变更前先问，小优化可直接执行
- 回复用中文，代码注释用英文，文档中英双语
- 极度节制 cargo 调用（系统负担）—— 默认不跑全量测试，高风险合并至多一次 `cargo check --lib`
- 单分支开发：所有工作直接在 main 分支
- 提交信息英文，格式 `<scope>: <description>`
```

- [ ] **Step 2: 删除文末重复的 Session Context 区块（行 342-350）**

将 CLAUDE.md 末尾「## 📝 Session Context」整节（含「### Memory Prompt」子节，行 342-350）连同其上的 `---` 分隔符一并删除——其中"项目/核心循环/语言"信息已被 Working Style 与 R6/R10 覆盖，Memory Prompt 行属会话运行机制、非项目宪法。

- [ ] **Step 3: 验证**

Run: `grep -c "My Working Style" CLAUDE.md`
Expected: `1`

Run: `grep -c "Session Context" CLAUDE.md`
Expected: `0`

- [ ] **Step 4: 提交**

```bash
git add CLAUDE.md
git commit -m "docs: consolidate language rules into My Working Style section"
```

---

### Task 9: 新增「Memory & Hooks」现状声明区块（经验 #6/#7，仅文字）

**Files:**
- Modify: `CLAUDE.md`（在「🏢 官方仓库」节后追加新区块）

**Interfaces:**
- Produces: 无下游消费。

- [ ] **Step 1: 在官方仓库节（行 338）后、文件末尾追加新区块**

```markdown
---

## 🧠 长期记忆与质量门 (Memory & Hooks)

- **长期记忆**: 走全局 `~/.claude/projects/.../memory/`（跨会话、Git 不追踪）。**不在项目内另造 MEMORY.md**——避免与全局记忆双源冲突。
- **质量门 (Hooks)**: 当前**未挂** `.claude/hooks/`。CLAUDE.md 里的规则目前靠模型遵守；未来如需"强制执行层"（如 PostToolUse → `cargo fmt`），在 `.claude/hooks/` 配置即可。
```

- [ ] **Step 2: 验证**

Run: `grep -c "Memory & Hooks" CLAUDE.md`
Expected: `1`

- [ ] **Step 3: 提交**

```bash
git add CLAUDE.md
git commit -m "docs: declare memory and hooks status"
```

---

### Task 10: 强化文档索引为 Context Tiers 并登记新文档（经验 #4）

**Files:**
- Modify: `CLAUDE.md:288-325`（文档索引）

**Interfaces:**
- Consumes: Task 1 的 `RELEASE.md`、Task 2 的 `PROCESS_MANAGEMENT.md`。

- [ ] **Step 1: 在「## 📚 文档索引」标题（行 288）下、表格前插入 Context Tiers 说明**

```markdown
> **Context Tiers**: Tier 1（每次加载）= 本 CLAUDE.md，项目是什么 + 怎么工作；Tier 2（按需加载）= 下表 `docs/reference/*`，Claude 工作时按主题自取；Tier 3（默认忽略）= `docs/archive/`、历史规格，除非明确要求不碰。
```

- [ ] **Step 2: 在文档索引表格中新增两行（RELEASE.md / PROCESS_MANAGEMENT.md）**

在表格末尾（GOOGLE_MEET_BRIDGE.md 行之后）追加：

```markdown
| RELEASE.md | [docs/reference/RELEASE.md](docs/reference/RELEASE.md) — 发版两步流程 + `just verify-build` 预检 + CI fail-fast 轮询 |
| PROCESS_MANAGEMENT.md | [docs/reference/PROCESS_MANAGEMENT.md](docs/reference/PROCESS_MANAGEMENT.md) — Singleton flock / Spec C 不变量 / CLI 写策略 |
```

- [ ] **Step 3: 修正文档索引中 HARNESS_PHILOSOPHY 的笔误（R11 → R10）**

CLAUDE.md 行 294 现写「（R11 详解）」，但红线只到 R10。改为「（R10 详解）」。

- [ ] **Step 4: 验证**

Run: `grep -c "Context Tiers" CLAUDE.md`
Expected: `1`

Run: `grep -c "RELEASE.md\|PROCESS_MANAGEMENT.md" CLAUDE.md`
Expected: `>=2`

Run: `grep -c "R11 详解" CLAUDE.md`
Expected: `0`

- [ ] **Step 5: 提交**

```bash
git add CLAUDE.md
git commit -m "docs: add context tiers and register new reference docs"
```

---

### Task 11: 新建 src/harness/CLAUDE.md（经验 #5 — R10 护栏前移）

**Files:**
- Create: `src/harness/CLAUDE.md`

**Interfaces:**
- Consumes: 内容全部从根 CLAUDE.md R10 段（行 61-84）提炼，不新造约束。

- [ ] **Step 1: 确认目标目录存在**

Run: `test -d src/harness && echo OK`
Expected: `OK`（若不存在则停下来报告——红线写的是 `src/harness/`，目录缺失需先与用户核对路径）

- [ ] **Step 2: 创建 `src/harness/CLAUDE.md`**

```markdown
# src/harness/ — 薄 Harness 护栏 (R10 本地红线)

> 本文是根 `CLAUDE.md` R10 的本地强化，编辑本目录前必读。完整哲学见
> [HARNESS_PHILOSOPHY.md](../../docs/reference/HARNESS_PHILOSOPHY.md)。

## 硬边界：12 文件 / ~4900 行

- 顶层 (8)：`mod.rs` / `agent.rs` / `deps.rs` / `trait_def.rs` / `callback.rs` / `chain_context.rs` / `trace.rs` / `trace_sink.rs`
- `agent/` 子目录 (4)：`think.rs` / `act.rs` / `guardrails.rs` / `prompt.rs`

**新增文件须在 PR 描述说明为何无法装进现有 12 个文件之一。**

## 加代码前必答 3 问

1. 这是脚手架还是认知？认知必须搬到 prompt。
2. 模型升级一档还需要它吗？不需要就删。
3. 现在有几个真实消费者？零个就撤回。

## 循环里的 5 个"不"

1. ❌ 不判断意图分类
2. ❌ 不做工具过滤 / 相关性评分
3. ❌ 不做完成度判断（除模型显式 stop）
4. ❌ 不做内容审查 / 安全打分
5. ❌ 不做错误恢复策略选择

任何"零现有消费者"的抽象立即撤回，绝不"为未来留口"。
```

- [ ] **Step 3: 验证内容与根红线一致**

Run: `grep -c "加代码前必答 3 问" src/harness/CLAUDE.md`
Expected: `1`

Run: `grep -c "12 文件" src/harness/CLAUDE.md`
Expected: `>=1`

- [ ] **Step 4: 提交**

```bash
git add src/harness/CLAUDE.md
git commit -m "docs: add local harness CLAUDE.md enforcing R10 guardrails"
```

---

### Task 12: 新建 src/gateway/CLAUDE.md（经验 #5 — 安全边界护栏）

**Files:**
- Create: `src/gateway/CLAUDE.md`

**Interfaces:**
- Consumes: 内容从根 CLAUDE.md Trust model 段 + 信任模型 note 提炼，不新造约束。

- [ ] **Step 1: 确认目标目录存在**

Run: `test -d src/gateway && echo OK`
Expected: `OK`（若不存在则停下报告路径问题）

- [ ] **Step 2: 创建 `src/gateway/CLAUDE.md`**

```markdown
# src/gateway/ — 安全边界护栏

> 本目录是 Aleph 的网络信任边界。改动认证 / 授权 / Origin 逻辑高风险，
> 编辑前必读。完整模型见 [SECURITY.md#auth-ux](../../docs/reference/SECURITY.md#auth-ux)。

## 信任模型 = 网络边界

- LAN-trust：无认证步骤，信任边界就是网络边界。默认只绑 `127.0.0.1`；
  `[gateway] host = "0.0.0.0"` 显式开放整个局域网。

## 两道护栏

- **device tier**（`method_authz.rs`）：远程 Panel 默认 **Chat tier**；对 Aleph
  自身配置的变更（`self_config` / `skill_install` / provider 配置 / `devices.*`
  等 config 类 RPC 与工具）须 operator，经 `devices.set_level` 显式提权；
  本机 (loopback) 始终 operator。
- **WS Origin 校验**（`origin_policy.rs`）：唯一保留的协议护栏，挡公网恶意网页
  跨源驱动 agent。域名部署须把 origin 加进 `[gateway] allowed_origins`。

## 红线

- 改认证 / 授权 / Origin 逻辑**必须同步更新测试**，不得只改实现。
- 不在 Gateway/Interface 层处理业务逻辑（R4：纯 I/O）。
```

- [ ] **Step 3: 验证**

Run: `grep -c "method_authz\|origin_policy" src/gateway/CLAUDE.md`
Expected: `>=1`

- [ ] **Step 4: 提交**

```bash
git add src/gateway/CLAUDE.md
git commit -m "docs: add local gateway CLAUDE.md for security boundary"
```

---

### Task 13: 终检——行数、宪法逐字不动、30 秒三问

**Files:**
- 只读验证，不改文件（除非发现回归）

- [ ] **Step 1: 行数在目标区间**

Run: `wc -l CLAUDE.md`
Expected: 210–250 之间。若 >250，回看哪些外迁块漏删；若 <210（不太可能），无需处理。

- [ ] **Step 2: R1-R10 / P1-P8 逐字未改**

Run: `git diff a6fa20229 -- CLAUDE.md | grep -E '^\-' | grep -E '### R[0-9]|### P[0-9]|严禁|原则|禁令'`
Expected: 无输出。若有输出，说明误删了红线/原则正文，必须还原。

（说明：`a6fa20229` 是本次优化前的 HEAD 基线。）

- [ ] **Step 3: 30 秒三问可答**

人工通读优化后 CLAUDE.md，确认能在 30 秒内回答：
1. 这是什么产品？（自托管个人 AI 助手，Rust Core + 多端）
2. 技术栈是什么？（Tech Stack 区块）
3. 新代码放哪里？（红线 R1-R4 + harness/gateway 本地 CLAUDE.md + 文档索引）

- [ ] **Step 4: 所有外迁文档均在 docs/reference 下**

Run: `git diff --stat a6fa20229 | grep -E '\.md' | grep -vE 'docs/reference/|src/harness/CLAUDE|src/gateway/CLAUDE|^ CLAUDE.md'`
Expected: 无输出（除根 CLAUDE.md、两个子目录 CLAUDE.md 外，所有新增/改动 .md 都在 docs/reference/）。

- [ ] **Step 5: 最终确认提交历史干净**

Run: `git log --oneline a6fa20229..HEAD`
Expected: 看到 Task 1-12 的 12 个 docs: 提交，无遗漏、无 WIP。
```
