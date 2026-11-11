# Static review — `src/extension`, `src/fetch`, `src/gateway` (Occam's Razor round, 2026-11-09)

> **入口分工**：本文件是本轮 review 的事实源；`AGENTS.md` 是 Pi / Codex 等非 Claude Code agent 的入口。内容一致。
>
> 范围：822 个 Rust 文件 / 413,548 LOC，**无 diff**（全量静态审查，不是 PR review）。
> 产出：32 个批次的 findings → 跨族裁决 → 分级路由 → 修复（直接提交到 `main`，无 PR）→ 一次统一 `cargo check`。

---

## 1. 范围与分批

32 个**文件互斥**批次（`batches.json` + `<BID>.md` 批次契约），由 `/tmp/partition.py` 生成并校验：
零重复、零遗漏，合计 822 文件 / 413,548 LOC。

| 批次 | 模块 | 文件 | LOC | 廉价档候选 | 深审发现 | 裁决（成立/降级/驳回） | 存活 P0/P1/P2/P3 |
|---|---|---:|---:|---:|---:|---|---|
| F1 | src/fetch — fetch provider contract (SSRF, redaction, size limits) | 5 | 380 | 21 | 5 | 0/3/2 | 0/2/1/0 |
| E1 | src/extension — lifecycle, loader, template, mod.rs (plugin lifecycle state machine) | 5 | 13,278 | 37 | 3 | 1/0/2 | 0/1/0/0 |
| E2 | src/extension — root services (plugin ops, visibility, state, watcher, service manager) | 27 | 11,135 | 42 | 2 | 1/1/0 | 0/1/1/0 |
| E3 | src/extension — hooks executor + hook types (ordering, consent, idempotency) | 20 | 13,201 | 50 | 3 | 2/1/0 | 1/2/0/0 |
| E4 | src/extension — manifest parsing/validation + effects (schema, fail-closed, effect claims) | 22 | 9,758 | 71 | 6 | 6/0/0 | 1/4/1/0 |
| E5 | src/extension — WASM runtime + registry/registrar + mcp_config (sandbox, single source of truth) | 20 | 8,888 | 43 | 6 | 5/1/0 | 2/4/0/0 |
| E6 | src/extension — marketplace install + panel publication (supply chain, publish path) | 12 | 3,543 | 40 | 5 | 4/1/0 | 4/1/0/0 |
| G1 | src/gateway — channel registry + resume coordinator + event visibility + delivery queue | 8 | 23,207 | 45 | 6 | 5/1/0 | 1/4/1/0 |
| G2 | src/gateway — session projector + visibility rules + i18n (single source of truth for state) | 9 | 11,879 | 31 | 3 | 3/0/0 | 1/0/2/0 |
| G3 | src/gateway — root part A (event bus, agent env, middleware glue, formatting) | 35 | 14,664 | 52 | 3 | 3/0/0 | 0/1/2/0 |
| G4 | src/gateway — root part B (misc root services) | 44 | 18,392 | 63 | 10 | 7/3/0 | 1/6/3/0 |
| H1 | src/gateway/handlers — part A (agent/users/projects/message CRUD handlers) | 21 | 14,665 | 0 | 8 | 7/1/0 | 1/5/2/0 |
| H2 | src/gateway/handlers — part B (session/config/provider handlers) | 22 | 14,698 | 0 | 9 | 9/0/0 | 3/4/2/0 |
| H3 | src/gateway/handlers — part C (tool/memory/misc handlers) | 22 | 14,696 | 0 | 7 | 5/2/0 | 1/6/0/0 |
| H4 | src/gateway/handlers — part D (auxiliary handlers) | 22 | 14,689 | 0 | 7 | 7/0/0 | 2/4/1/0 |
| H5 | src/gateway/handlers — teams + session db_handlers + memory + providers | 24 | 15,233 | 0 | 18 | 17/1/0 | 2/15/1/0 |
| H6 | src/gateway/handlers — graph, cron, plugins, generation_providers, search_config, extensions, security_config, config, auth | 34 | 9,489 | 0 | 10 | 6/4/0 | 2/6/2/0 |
| IF1 | src/gateway/interfaces — telegram (+streaming) channel adapter | 25 | 8,232 | 0 | 20 | 14/4/2 | 0/15/3/0 |
| IF2 | src/gateway/interfaces — discord + voice_call adapters (incl. relay tests) | 26 | 12,304 | 0 | 13 | 8/5/0 | 3/8/2/0 |
| IF3 | src/gateway/interfaces — feishu + whatsapp adapters (runtime/policy/auth splits) | 54 | 11,537 | 0 | 21 | 21/0/0 | 4/14/3/0 |
| IF4 | src/gateway/interfaces — imessage + matrix + line + xmpp adapters | 40 | 10,751 | 0 | 35 | 24/10/1 | 8/20/6/0 |
| IF5 | src/gateway/interfaces — slack + qq + wechat + nostr adapters | 42 | 9,991 | 0 | 22 | 20/2/0 | 4/13/5/0 |
| IF6 | src/gateway/interfaces — sms, irc, signal, email, mattermost, webhook + root | 23 | 12,225 | 0 | 16 | 13/3/0 | 5/7/4/0 |
| EE1 | src/gateway/execution_engine — part A (execute core, tool dispatch) | 19 | 12,940 | 0 | 4 | 4/0/0 | 1/2/1/0 |
| EE2 | src/gateway/execution_engine — part B + run_loop (streaming, cancellation) | 26 | 21,248 | 0 | 2 | 2/0/0 | 0/1/1/0 |
| EE3 | src/gateway/execution_engine — tests + slash command body + skill pregrant + btw_promote | 14 | 6,432 | 0 | 4 | 3/1/0 | 1/1/1/1 |
| GX1 | src/gateway — server + control_plane + admin_api (IPC surface, R4/R6 boundary) | 30 | 16,873 | 0 | 11 | 10/1/0 | 4/3/4/0 |
| GX2 | src/gateway — telecom + inbound_router (routing, R5 delivery, R4 pure I/O) | 27 | 20,563 | 0 | 8 | 3/0/5 | 0/1/1/1 |
| GX3 | src/gateway — session_store + session_manager + busy_queue (state ownership, backpressure) | 24 | 17,556 | 0 | 7 | 3/0/4 | 0/2/1/0 |
| GX4 | src/gateway — pty + event_emitter + events (process IO, event fanout) | 31 | 13,248 | 0 | 12 | 11/1/0 | 1/5/6/0 |
| GX5 | src/gateway — security + middleware + surface + link + message_assembly + pipeline | 39 | 9,593 | 0 | 3 | 0/1/2 | 0/0/0/1 |
| GX6 | src/gateway — voice + openai_api + btw + mcp_face + agent_env + formatter | 50 | 18,260 | 0 | 17 | 16/1/0 | 2/13/2/0 |

分批原则：同目录/同子系统的文件放同一批，批内文件互斥；单批规模 3.5K–23K LOC；
安全敏感面（`src/extension/runtime/wasm`、`src/extension/marketplace`、`src/gateway/handlers`）单独成批。

---

## 2. 方法：两个模型档位 + 跨族裁决

> 用户要求的 "Jev 档" 在本机**不存在同名模型**：magpie 只提供 `minimax-cn` / `88apigpt` / `88apiclaude` /
> `kimi` / `codex` 五个 provider（已核实 `/home/zou/.config/magpie/providers.json`）。
> 因此 "Jev 档" 按**角色**落地为**本机最便宜的可用模型** `minimax-cn/MiniMax-M3.1-Flash`，
> 下表逐条标出哪些步骤走了该档。

| # | 步骤 | 档位 | 模型 | 说明 |
|---|---|---|---|---|
| 1 | 批次划分 / 文件互斥校验 / 批次契约生成 | 脚本 | — | `/tmp/partition.py`（确定性） |
| 2 | **候选线索扫描**（全量逐文件阅读，输出候选表） | **Jev 档（廉价）** | `minimax-cn/MiniMax-M3.1-Flash` | 32 批，64k 输出预算 |
| 3 | **候选表复核 + 覆盖率缺口检查** | **Jev 档（廉价）** | `minimax-cn/MiniMax-M3.1-Flash` | `candidates/<BID>.gap.md` |
| 4 | **深度审查**（逐文件全文阅读，判定 + 最小修复建议） | 推理档 | `88apigpt/gpt-6.1-sol`（后段补齐改用 `88apiclaude/claude-sonnet-5-5` 走 raw API） | 32 批 |
| 5 | **跨族裁决**（复核/驳回/降级） | 推理档 | `88apiclaude/claude-sonnet-5-5` + `minimax-cn/MiniMax-M3` | 两个族各裁一次，分歧交第三族 `claude-opus-5-5` |
| 6 | **分级与路由**（FIX-NOW / DEFER / REJECT…） | **Jev 档（廉价）** | `minimax-cn/MiniMax-M3.1-Flash` | `classification/<BID>.class.md` |
| 7 | 修复实现（编辑源码，不提交） | 廉价档 | `minimax-cn/MiniMax-M3.1-Flash`（fable 别名） | worktree 内按批并行，见 §2.1 |
| 8 | **修复复核**（逐条判定 FIXED/PARTIAL/REGRESSION） | 推理档 | `88apiclaude/claude-sonnet-5-5` | `fixes/FIX-<BID>.review.md` |
| 9 | 提交 / 合并 / 统一 `cargo check` | 编排 | — | 主会话 |

### 2.1 中途的额度事故与档位切换（必须记录）

第一轮修复代理用的是 Agent 工具的 `opus` 别名（= `88apigpt/gpt-6.1-sol`）。约 11:49 该 provider
返回 HTTP 403 `余额不足：当前余额 ¥0.05`——**额度耗尽**，十余个深审/修复代理在执行中途被杀，
在工作树里留下 14 个文件的**半成品修改**。随后 13:06 又撞上两层限流：bili 本地代理的
`429 … holding outbound sends`（并发过高）与 MiniMax 中国区 Token Plan 上限（15:00 后恢复）。

处置：

- 探测七个 provider 的真实可用性（证据见本目录 `evidence/`），确认 `88apigpt` 仅能跑极小请求；
- 发现 Agent 工具的 `sonnet`/`opus`/`haiku`/`fable` 四个别名中，**`fable` → `minimax-cn/MiniMax-M3.1-Flash`**（有额度），
  修复代理全部改用 `fable` 档续跑（**这 4 个批次属"廉价档"执行修复，不是推理档**）；
- 尝试用 `affinity.json` 把 `sonnet`/`opus` 别名改指 `88apiclaude` —— **失败**：代理把映射表缓存在内存并用自己的值回写，
  别名仍落到 `88apigpt`（见 §2 的别名说明）；
- 剩余 5 个批次的深审改走 **raw magpie API**（`/tmp/magpie_batch.py`），用 `claude-sonnet-5-5` 直接跑，
  单批 44–238 s 跑完，比 Agent 工具更快也更省；
- 每个修复代理的提示词都加了"**上一轮可能留有半成品，先检查再继续**"，并要求**先建 fix note 再逐条追加**，
  因此再次中断也不会丢进度；
- 修复质量由 §8 的**推理档 diff 复核**兜底（`/tmp/review_fix.py`）：把每批的真实 `git diff` 交给
  `claude-sonnet-5-5` 逐条判定 FIXED / PARTIAL / NOT_FIXED / REGRESSION。

> 净效果：审查与裁决由推理档完成；修复实现这一档因额度原因落在廉价档，
> 用推理档复核兜底——这是本轮与常规流程的**唯一偏离**，在此明确记录。

**为什么扫描档不是推理档**：廉价档在本机只有 Flash 一个候选，而 Flash 会先花 thinking token；
`max_tokens=24000` 时 13/15 批的输出被 thinking 吃光（`thinking_tokens=23999/24000`，正文为空）。
把预算提到 64k 后 Flash 正常产出完整候选表；在此之前，扫描由 `minimax-cn/MiniMax-M3` 兜底跑完
（证据：`evidence/cheap-tier-flash-pilot.log`）。**推理档（gpt-6.1-sol / MiniMax-M3）只做需要判断的部分**。

**Agent 工具的模型别名全部塌缩到同一个后端**：`haiku` / `sonnet` / `opus` / `fable` 四个别名
在 `/home/zou/.config/magpie/affinity.json` 里都指向 `88apigpt/gpt-6.1-sol`（已实测）。
所以"用 subagent 做廉价档"在本机做不到——廉价档只能走 raw magpie API。
这也是第 2/3/6 步走脚本而不是 Agent 工具的原因。

### 2.2 第二次额度事故、fable 档停摆与 region-splice 工具（必须记录）

第 2 波修复代理改用 `opus` 别名（仍是 `88apigpt/gpt-6.1-sol`）。约 16:17 该 provider 再次归零
（`403 余额不足：当前余额 ¥0.01`，随后 `¥-0.22`），紧接着 `88apiclaude` 也归零
（`88apiclaude: 余额不足：当前余额 ¥-0.14`）。当时仍可用的只剩
`kimi/kimi-for-coding`、`minimax-cn/MiniMax-M3`、`minimax-cn/MiniMax-M3.1-Flash`
（`codex/gpt-5.6-terra` 是 usage limit）。`affinity.json` 由代理进程自己回写，
无法把别名改指到有额度的 provider。

处置与代价：

- 4 个改用 `fable` 档（= `minimax-cn/MiniMax-M3.1-Flash`）的修复代理（H6 / F1 / GX5 / GX3）
  **运行 30 分钟只输出叙述、零文件改动**（diff 与 mtime 全部冻结），被编排侧杀掉；
  这 4 批改由 raw-API 通道完成（H6 3/3、F1 1/1、GX5 1/1、GX3 1/1、IF4 20/20）。
- 为绕开 Agent 工具的 provider 限制，第 3 波起修复改走 **raw magpie API + region splice**
  （`/tmp/region2.py` → `region4.py`）：把模型重写的 ±30 行区域拼回原文件。
  **该工具不解析 Rust**，造成 9 个文件语法损坏、35 处编译错误、约 15 处静默语义删除——
  全部已修复，处置清单见 `evidence/splice-damage-repair.md`。
- 事故的直接原因是**并发**：本机一次最多 20 个 subagent，而 88apigpt / 88apiclaude 是共享额度，
  高并发把余额在几十分钟内打穿。下次同类工作应把并发压到 provider 余额能承受的量级，
  或一开始就按"provider 可用性"而不是"Agent 工具别名"来编排。

> 净效果：审查、裁决、diff 复核由推理档完成；**修复实现**这一档因额度原因先后落在廉价档与
> "blind splice"档，因此必须有 §4.4 的损伤审计与 §5 的编译门兜底。

### 裁决规则（`/tmp/final_verdicts.py`）

- 两族都 UPHELD → **UPHELD**
- 两族都 REFUTED → **REFUTED**（记录两族理由，从修复清单中剔除）
- 一族 UPHELD、一族 REFUTED → 交第三族 `88apiclaude/claude-opus-5-5` 终裁（终裁者看到 finding + 两族理由 + 重新读取的源码片段）
- 任一族 DOWNGRADE → **DOWNGRADE**，severity 取两族中较低者

裁决时重新从磁盘读取被指控位置 ±16 行源码，**不采信审查者的转述**。

---

## 3. 发现与分级

深审在 **32 批 / 822 文件 / 413,548 LOC** 上产出 **306 条发现**（声称 P0=62 P1=172 P2=69 P3=3）；跨族裁决后 **288 条存活**（P0=55 P1=171 P2=59 P3=3）。

| 严重级 | 深审声称 | 裁决后存活 | 典型形态 |
|---|---:|---:|---|
| P0 | 62 | 55 | 凭据明文外泄 / 签名未校验导致冒充 / 授权绕过 / 越界路径读写 / 沙箱逃逸 |
| P1 | 172 | 171 | 错误被吞或被伪装成成功 / 取消与超时语义不失效 / 竞态覆盖 / 阻塞调用卡住 runtime / 协议被破坏 |
| P2 | 69 | 59 | 无界分配或无界循环 / TOCTOU / 资源泄漏 / 锁粒度过大 / 重试与幂等语义不一致 |
| P3 | 3 | 3 | Occam 债务：多余概念、重复实现、无用抽象 |

### 存活的 P0（全部）

| ID | 位置 | 失败形态 |
|---|---|---|
| E3-01 | `src/extension/hooks/executor.rs:253–256` | 已批准的命令为 `sh hooks/lint.sh`，审阅及指纹绑定的文件是插件根目录下的 `hooks/lint.sh`。通过公开的 `execute_interceptors` 入口传入一个 `working_dir` 不同于插件根目录的 `HookContext`，且该目录存在内容不同的 `hooks/lint.sh` → consent 仍校验插件根目录中未变的已批准文件，实际进程却在显式目录执行另一个脚本。命令模板、owner |
| E4-01 | `src/extension/manifest/adapters/cursor.rs:178` | 加载一个包含 `.cursorrules` 符号链接的插件，链接指向插件根目录外、服务进程可读的敏感文本文件 → `read_to_string` 跟随链接，将该文件全文作为 `SkillRegistration.content` 放入返回的 capability。`.cursor/rules` 本身指向外部目录，或其下的 `.md` / `.mdc` 文件指向外部文件，也有相同效果。无需竞态或执行插件代码，即可越过其他组件解析器实施的 |
| E5-03 | `src/extension/runtime/wasm/credential_injector.rs:173` | A `UrlPath` binding with placeholder `api` targets `https://api.example.com/v1/data`; after the binding resolves, a secret value such as `evil` produces `https://evil.example.com/v1/data` (or a placeholder in another URL |
| E5-04 | `src/extension/runtime/wasm/host_functions.rs:303` | A credential is injected into a query URL, then DNS, TLS, timeout, or connection failure occurs → reqwest formats the URL containing the secret into the error, `do_http_fetch` serializes that message, and the untrusted W |
| E6-01 | `src/extension/marketplace/names.rs:27` | 调用 `MarketplaceManager::remove(".")` → `removal_refusal` 接受该名称，即使根本没有这个 registration 也继续删除 → `cache_dir.join(".")` 指向缓存根，`remove_dir_all` 递归删除全部 marketplace 的内容，包括本应不可删除的 `aleph-official`。某些平台最终删除 `.` 本身会返回错误，但已经删除的子目录不会 |
| E6-02 | `src/extension/marketplace/installer.rs:238` | 原始树包含 `main.py`（内容 `#`）、`payload.txt`（内容 `\nprint("injected")\n`）和一个不变的 `plugin.toml`；将 `payload.txt` 删除，并把 `main.py` 改为 `#payload.txt\nprint("injected")\n` → 两棵树排序后喂给 SHA-256 的字节流完全相同，但原先只含注释的入口现在执行注入代码。`verify_plugin_i |
| E6-03 | `src/extension/marketplace/mod.rs:665` | marketplace 仓库含 `alias` symlink，指向 marketplace 外某个已存在插件目录的父目录，条目声明 `source = "./alias/existing-plugin"` 且不带可选 hash → 组件过滤认为路径安全。最终 `source_path` 的叶子 `existing-plugin` 是真实目录，因此 `copy_dir_recursive` 的 `symlink_metadata(src |
| E6-04 | `src/extension/marketplace/installer.rs:157` | 先通过公开 `install_plugin_from_cache` 安装名为 `.tmp-install-victim` 的插件，再安装 `victim` → 第二次调用把前者的正式安装路径当作“陈旧 staging”递归删除。另一条同样确定的路径是先安装 `.bak-victim`，随后更新 `victim` → 更新在备份前无条件删除 `.bak-victim`。这些名称均通过共享名称守卫；无需并发、symlink 或异常 I/O。 |
| G1-01 | `/home/zou/data/workspace/Aleph/src/gateway/event_visibility.rs:1171` | An event's cache miss reads a personal session's old metadata, then pauses before inserting it. `projects.channel.bind` commits the move into a project room and calls `forget_session`; because the miss has not inserted a |
| G2-01 | `src/gateway/projection_reconciler.rs:476` | Boot finds an unregistered forked epoch belonging to an attributed personal/project session, but `session_store.get_metadata(parent)` returns an error → `persisted_attribution` converts that error to `None`, and healing  |
| G4-01 | `/home/zou/data/workspace/Aleph/src/gateway/reply_emitter/sanitize.rs:128` | 输入为一个 fenced code block，内容含一个单独的反引号及 `<think>x</think>`（例如四反引号引用内的字符串 ````"```text\n` <think>x</think>\n```"````）→ quick probe 启动扫描；到达 fence 内那个单反引号时，三反引号分支不匹配，inline 分支又被 `!in_fenced_code` 排除。下面的复制循环复制空串，`continue` 后索引与 |
| H1-01 | `src/gateway/handlers/hooks_admin.rs:213` | 两个并发 RPC（或 RPC 与工具调用）在不同 runtime worker 上读取同一版 hooks，然后分别追加/删除并写回 → 后写入者覆盖前一个已提交的修改。即使两次 rename 不重叠、两个调用都成功，新增的 hook 仍会丢失；add/remove 交错还会复活已删除项。所有写入者另共用一个临时文件，写入重叠时还会互相截断临时内容或令另一方 rename 失败。这不是 atomic rename 能解决的 read-mo |
| H2-01 | `src/gateway/handlers/markdown_skills.rs:238` | `skills.install` 传入内容有效、文件名为 `...zip` 的本地归档，例如 `/tmp/...zip` → `file_stem()` 得到 `..`，目标成为 `<config_dir>/skills/..`。代码在扫描第三方内容之前递归删除该目标，因而会删除 skills 以外的配置、凭据等同级文件。即使最后删除 `..` 本身失败，其子项也可能已经删除，且错误被忽略。文件名 `..zip` 则生成 `.`，删除整 |
| H2-02 | `src/gateway/handlers/fetch_config.rs:307` | 更新请求中的 backend 带 `api_key: "secret"`，且配置保存成功 → handler 虽然将持久配置中的 key 清空，却序列化仍持有原始 key 的入站 DTO 并发布 `ConfigChanged`。该事件的 `value.backends[*].api_key` 因而包含明文凭据，违反 DTO 自身的 “Inbound only (never echoed)” 契约。甚至被配置循环跳过的 Firecrawl |
| H2-03 | `src/gateway/handlers/pairing.rs:181` | 启用该模块的 debug tracing 后调用 `pairing.approve` 或 `pairing.reject` → 在 store 执行操作之前，完整配对 code 被写进日志。操作报错时 code 还可能保持有效；即使操作成功，也已经把明确标为 credential material 的值复制到了日志保留域。批准路径的审计注释称 code 从不记录，但此前的 debug 调用实际记录它。 |
| H3-01 | `src/gateway/handlers/gateway_ticket.rs:167` | 已获授权的邀请方调用 `gateway.ticket.create`，希望绑定一个用户，却提交 `{"user_id":42}`、布尔值或对象 → `as_str()` 失败被当作未提供用户，完全跳过 active-user 检查，并成功创建 UNBOUND 票据。本文件明确规定 UNBOUND 票据的兑换者成为 owner/operator（146–150 行）；因此一个无效的身份绑定请求会得到权限更高、而非被拒绝的邀请。这里的问题不 |
| H4-01 | `/home/zou/data/workspace/Aleph/src/gateway/handlers/mcp_config.rs:512` | actor store 已有名字为 `a:b`、ID 为 `a_b` 的服务，旧配置还有不同服务 `a b` → `derive_server_id` 同样得到 `a_b`。迁移只比较 ID，因此跳过导入却把 `a b` 加入 `migrated`，随后从 `unified_tools.mcp` 删除并持久化；旧服务的 command、args、env 等配置未经迁移就丢失。这个路径不是重跑同一个服务的正常幂等去重。 |
| H4-02 | `/home/zou/data/workspace/Aleph/src/gateway/handlers/secret_migration.rs:145` | 例如 `generation.image_providers.local.api_key = IMAGE_KEY`，同时 `generation.video_providers.local.api_key = VIDEO_KEY` → 遍历两个独立 map 时都写 `gen:local`，后一项覆盖前一项，且两次都计作迁移成功。保存 generation 段会去掉这些 `skip_serializing` 的 api_key；vault |
| H5-01 | `/home/zou/data/workspace/Aleph/src/gateway/handlers/session/db_handlers/checkpoint.rs:223` | Create checkpoint C, append further messages, then call branch with `new_session_key` equal to the source key (or another existing visible session). The target check permits it. The file backend rewrites that target's tr |
| H5-02 | `/home/zou/data/workspace/Aleph/src/gateway/handlers/providers/handlers.rs:507` | Delete a non-default provider whose API key exists only in the vault. Vault deletion succeeds but config saving fails, for example because the config directory is not writable. The RPC returns an error and the old disk c |
| H6-01 | `/home/zou/data/workspace/Aleph/src/gateway/handlers/search_config/update.rs:222-227` | 客户端提交带真实 `backends.<name>.api_key` 的搜索配置 → 成功处理后，将包含明文密钥的输入 DTO 整体作为 `config.changed` 的 value 发布。订阅配置事件的其他客户端和内部订阅者收到密钥，而不是仅收到“已更新”通知；事件发布的 Debug preview 还会格式化包含密钥的整个 frame。Vault 持久化和 get 响应的脱敏均不能阻止这一出口。 |
| H6-02 | `/home/zou/data/workspace/Aleph/src/gateway/handlers/generation_providers/handlers.rs:387-401` | 删除一个非默认 provider，其 Vault 可写但 config.toml 不可写、解析失败或增量保存拒绝外部变更 → handler 先修改共享配置并持久删除 Vault 密钥，再因配置保存失败返回错误。磁盘仍保留 provider，凭据却已永久丢失；失败的删除还使进程中的 Config 与磁盘、未收到变更事件的运行时 registry 不一致。重试不能恢复原密钥。 |
| IF2-01 | `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/config.rs:12` | 对含真实令牌的配置执行 `format!("{config:?}")` 或 tracing 的 debug-field 格式化 → 派生实现直接输出完整 `bot_token`。这直接违反本批次明确规定的“secret 不得进入 Debug”契约；不假定当前存在某条生产日志调用。 |
| IF2-02 | `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/mod.rs:1297` | 以不同配置实例 ID 注册两个 Discord bot，例如 work/personal → factory 都创建 ID 为 `discord` 的 channel，入站普通消息、slash 与按钮事件也都标记为 `discord`。registry 按实际 channel.id 发布，故两个账号不能保留独立身份，后者进入前者的替换/路由身份，而不是配置指定的实例。直接调用 `DiscordChannel::new(custom_id |
| IF2-03 | `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/draft.rs:243` | `DraftChunker::default().chunk(&"x".repeat(4000))` → 先消费两个各 2000 字符的 body，再按 marker 缩减并截断；continuation marker 为 12 字符、end marker 为 6 字符，helper 又各预留一次，最终只保留 1976 + 1988 = 3964 个原文字符，丢失 36 个，且不会进入下一块。另一个独立触发是空 marker、cap=1 |
| IF3-01 | `src/gateway/interfaces/feishu/feishu_inbound/webhook_server.rs:141-161` | With an Encrypt Key configured, submit a plaintext event containing the valid verification token but no `encrypt` field or signing headers → the event bypasses signature verification and timestamp freshness and can enter |
| IF3-02 | `src/gateway/interfaces/feishu/feishu_inbound/mapper.rs:62-80` | An otherwise DM-approved user clicks an actionable card in a group whose messages are disabled or not allowlisted → the action is mapped with `is_group: false`, bypassing the group gate and mention requirement. The centr |
| IF3-03 | `src/gateway/interfaces/whatsapp/wa_auth/vault_store.rs:60-79,119-124` | Call the public encrypted save/load API with real binary auth data, e.g. `creds_blob: vec![0xff]` → UTF-8 replacement changes serialized bytes before encryption; save returns success but load cannot reconstruct the crede |
| IF3-04 | `src/gateway/interfaces/whatsapp/mod.rs:409-418,439-468` | Set a nonempty `allowed_chats` restricting the channel to chat A, while the sender/chat otherwise passes the configured access and central authorization policies → a message from chat B is still forwarded. The adapter on |
| IF4-01 | `src/gateway/interfaces/xmpp/message_ops/ops.rs:181` | 使用默认 `use_tls=true` 连接服务器 → 实际直接拆分裸 TCP，在收到 features 后发送包含用户名和密码的 SASL PLAIN；没有 STARTTLS、TLS 握手或证书验证。网络观察者取得可直接解码的凭据；要求 TLS 的正常服务器则无法登录。 |
| IF4-02 | `src/gateway/interfaces/xmpp/message_ops/xml_helpers.rs:40` | 合法 stanza `<message from='attacker@example.org' type='chat'><body>from="victim@example.org" type="groupchat"</body></message>` → helper 在整个 XML 中优先找双引号属性，得到正文中的 victim/groupchat，而不是根元素上的真实属性。转换后 sender/conversation/group |
| IF4-03 | `src/gateway/interfaces/line/config.rs:38` | 对任一已填入真实凭据的配置执行 Debug 格式化（例如排障日志）→ token、LINE signing secret、XMPP/BlueBubbles password 原文进入输出。这里不声称生产当前必然记录了配置；泄密发生在公开 Debug 契约本身，明确违反本批 secret hygiene 约束。 |
| IF4-04 | `src/gateway/interfaces/imessage/bluebubbles/api.rs:188` | 发送到某个电话号码/邮箱，查询结果中先出现包含此联系人的群、后出现其私聊 → `matches_part` 在群上成立，立即返回并缓存群 GUID。原本的私人内容随后发送给整个群；缓存使后续发送持续错误。 |
| IF4-05 | `src/gateway/interfaces/imessage/db.rs:281` | 一批中先有合法行 A，随后某行 B 的列类型/非空约束不满足 `row.get` → A 已使 `last_message_rowid` 前移，但 B 的 `?` 丢弃整个已收集 Vec。调用方把错误变成空 Vec，却继续持久化前移水位；后续 `ROWID > watermark` 永远不再取到 A。 |
| IF4-06 | `src/gateway/interfaces/imessage/bluebubbles/api.rs:458` | 离线期间同一 `dateCreated` 毫秒积累超过 100 条记录 → 查询只返回按时间升序的前 100 条，水位直接提交该毫秒；下次查询 created-after 水位时排除其余同时间记录。恢复路径没有分页 offset 或稳定的同时间 tie-breaker，丢失剩余消息。 |
| IF4-07 | `src/gateway/interfaces/line/config.rs:58` | 配置 `group_policy=disabled`，群成员发送包含 `@aleph` 的消息 → adapter 直接转发；Core 收到的却是默认 Open/require-mention，因此仍允许该消息。`dm_policy=disabled` 对已经配对的用户也同样无效，非空 allowed_users/allowed_groups 不被消费。不是“空 allowlist 开放”的争议，而是明确禁用/限制配置没有生效。 |
| IF4-08 | `src/gateway/interfaces/matrix/outbound.rs:270` | new_text 超过 MATRIX_MSG_LIMIT 被分成多片 → 函数只取 chunks[0]，以 replacement event 覆盖原消息并返回成功，后面的文本被静默舍弃。调用者没有任何截断提示。 |
| IF5-01 | `src/gateway/interfaces/nostr/message_ops/ops.rs:162–178` | 不可信 relay 发送任意内容，将 `pubkey` 填成 allowlist 中的用户，按公开算法重算 `id`，将 `sig` 填成任意非空字符串 → 哈希检查与 allowlist 均通过，内容以被冒充用户的身份进入 agent。攻击不需要该用户私钥；不是“修改 pubkey 却保留旧 id”的情形。 |
| IF5-02 | `src/gateway/interfaces/nostr/message_ops/protocol.rs:59–79` | 向非 `public` conversation 发送包含私密信息的回复 → 生产路径构造 kind-4 event，却把正文原样放入 `content` 后广播给 relay。接收端无法按 NIP-04 解密，relay 及获得事件的第三方可直接读取正文；签名不提供保密性。 |
| IF5-03 | `src/gateway/interfaces/slack/config.rs:16–22` | 对任一配置做合法的 `format!("{:?}", config)`、调试输出或错误上下文打印 → 原样输出 Slack app/bot token、QQ client secret、WeChat token 或 Nostr 私钥。这直接违反批次的“凭据不得出现在 Debug impl”契约；不需要假定已经存在某个生产日志调用。 |
| IF5-04 | `src/gateway/interfaces/wechat/inbound/mapper.rs:84–120` | 普通用户消息 `msg_type=1, to_user_id=account_id` 且没有 room → policy 按私聊放行，mapper 却推断为 group，并把 conversation ID 设为机器人账号，而非发送人；Alice/Bob 因而共享 conversation，context token 也写到同一键。另一个确定性错误是 typed Message 序列化后 `room_id: null` 让 `Value |
| IF6-01 | `src/gateway/interfaces/email/config.rs:140-146` | 配置允许 `boss@company.com` 或已有测试支持的 `@company.com` → `boss@company.com.attacker.example` 同样命中子串，虽实际邮箱域名由攻击方控制，仍通过邮件适配器的 sender allowlist。调用方 `src/gateway/interfaces/email/message_ops.rs:374-378` 直接以此决定是否发布入站消息；这里证明的是该 allow |
| IF6-02 | `src/gateway/interfaces/email/message_ops.rs:346-356` | `UNSEEN` 找到一批邮件 → 使用会隐式置 `\\Seen` 的 `RFC822` FETCH 读取正文；若 fetch 流中途出错，错误又被 `r.ok()` 丢弃，或者 inbound receiver 已关闭，部分邮件从未成功发布。随后代码仍对整个 search set 执行 `+FLAGS (\\Seen)`。下一轮只搜 `UNSEEN`，这些未处理邮件不再重取。即使仅修复末尾 STORE，前面的 RFC822 读取仍会提 |
| IF6-03 | `src/gateway/interfaces/email/config.rs:29-49` | 对任一配置执行正常的 `format!("{config:?}")` 或调试日志 → 派生的 Debug 输出完整邮件密码、NickServ 密码或 Mattermost bot token。三个类型均直接派生 Debug，违反本批次明确要求的 secret-bearing Debug 禁令；不需要假设上游错误响应会恰好回显秘密，也不声称已观察到生产日志泄露。 |
| IF6-04 | `src/gateway/interfaces/signal/message_ops.rs:328-346` | Signal 同一配置实例中两个不同发送者发送毫秒时间戳相同的消息，或 IRC 同一发送者的两条缓冲消息在同一毫秒被处理 → 两条消息的 channel ID 和 message ID 都相同。跨模块确认 `src/gateway/inbound_router/mod.rs:612-624` 的 key 只有 `channel_id:message_id`，第二条会被当作重复而丢弃，并不包含 sender/conversation 来补 |
| IF6-05 | `src/gateway/interfaces/irc/mod.rs:250-262` | 注册 `email-work` 与 `email-personal` 等两个合法同类型实例 → 四个工厂只实现 create，并把 runtime ID 固定成类型名。跨模块确认 `crates/aleph-slot-api/src/channel.rs:1187-1200` 的默认 create_with_id 会忽略传入 id；`src/gateway/channel_registry.rs:475-487` 随后以返回的 chan |
| EE1-01 | `src/gateway/execution_engine/unattended_redacting_sink.rs:113` | An unattended run encounters a tool error containing a recognizable credential, such as `AKIAIOSFODNN7EXAMPLE` → the individual `ToolCallCompleted.result.error` is masked, but the same original error survives in `Session |
| EE3-01 | `src/gateway/execution_engine/slash_skill_pregrant/mod.rs:188-204,216-227` | 已有两个用户级根中的同名 skill：低优先级的正常文件为 catalog 注册了允许 `bash` 的行，高优先级目录的 `SKILL.md` 则是指向项目内文件的符号链接。扫描器跳过该链接，但 `ReadSkillTool::load_candidates` 仍把其目录列为第一候选。这里仅 canonicalize/检查目录，所以两个目录均被判作 operator-owned；随后 `parse_skill_file(loaded. |
| GX1-01 | `/home/zou/data/workspace/Aleph/src/gateway/server/attachment_route.rs:42-79` | An HTTP caller who knows a session/attachment identifier requests `/api/memory/sessions/attachment/{session_id}:{attachment_id}` without credentials → receives private attachment bytes, including another user's attachmen |
| GX1-02 | `/home/zou/data/workspace/Aleph/src/gateway/server/attachment_route.rs:92-107` | A stored attachment contains HTML or script-bearing SVG and its corresponding MIME type; a user opens its download URL → the response renders as an active document on the gateway origin. Unlike the artifact/canvas routes |
| GX1-03 | `/home/zou/data/workspace/Aleph/src/gateway/server/connection/mod.rs:286-288` | Debug logging is enabled and a valid `connect` frame puts `token`, `device_token` or `bootstrap_ticket` within its first 200 bytes → the credential is written verbatim to application logs before authentication or method  |
| GX1-04 | `/home/zou/data/workspace/Aleph/src/gateway/server/connection/dispatch.rs:167-186,210-244` | An admin submits an admin-only ordinary RPC while its lane is occupied, then `users.update` demotes that user before the permit becomes available → dispatch still scopes the captured operator snapshot and the Core admin  |
| GX4-01 | `src/gateway/event_emitter/redacting.rs:94` | unattended run 输出两个 delta：`sk-` 和 `ABCDEFGHIJKLMNOPQRSTUVWX` → 每段单独都不匹配 API-key 规则，故两段原样进入事件总线。第二帧的 `full_text` 即使已脱敏，也不能收回第一帧，且该帧自己的 `delta` 仍是明文；按 delta/content 拼接的客户端能恢复完整密钥。Instant 模式也未避开此问题：内层 planner 累积的是这些已经逐段“脱敏” |
| GX6-01 | `/home/zou/data/workspace/Aleph/src/gateway/mcp_face/http.rs:208` | 用户 A 初始化 session 后，持有另一份有效凭据的用户 B 一旦知道 A 的 session ID，就能对它发 GET 接管通知连接，或发 DELETE 删除它。HTTP admission 只证明 B 有权进入 MCP face，不证明这个 session 属于 B；POST 同样只按 ID 取 session。`initialize` 没有把已认证 caller 传给 session 创建，表内也没有 owner，因此这不是 |
| GX6-02 | `/home/zou/data/workspace/Aleph/src/gateway/voice/streaming/mod.rs:83` | 对包含真实凭据的公开 `StreamingTarget` 执行正常的 Debug 格式化，例如 `format!("{target:?}")` → 派生实现逐字段输出原始 `api_key`。这直接违反本批明确的“secret 不得出现在 Debug impl”契约；确认的是可调用的 Debug 输出泄密，不声称已观察到生产日志泄漏。 |

### 被驳回 / 降级的发现（记录在案）

| ID | 批次 | 终裁 | 理由 |
|---|---|---|---|
| F1-01 | F1 | REFUTED（P0） | Line 112-122 shows bearer token passes through `send()` with `Some(self.api_key.as_str())` for redaction; upstream error never contains the key / Line 47 explicitly documents verbatim upstream errors; the trigger depends on Firecrawl echoing the bearer token, not shown reachable. |
| F1-04 | F1 | REFUTED（P0） | Line 42 wraps inner error in provider variant; crawl4ai backend constructed at line 27 from config already handles its own token / Line 42 forwards backend errors verbatim, but the excerpt does not establish that the configured token can appear in those errors; the trigger is hypothetical. |
| F1-02 | F1 | DOWNGRADE（P1） | Line 68-89 constructor accepts only `base_url` and `api_key`, calls no-argument `build_client()`, ignores configured timeout / Line 68 confirms no timeout parameter, but the excerpt does not show `build_client()` ignoring configured timeouts or constructor reachability. |
| F1-03 | F1 | DOWNGRADE（P1） | Line 114 string concatenation `format!("{}/v2/scrape", self.base_url)` mangles URLs with query/fragment / Line 114 misplaces the appended path after a query, but this requires an unusual base URL with query/fragment and causes a wrong request, not severe impact. |
| F1-05 | F1 | DOWNGRADE（P2） | Line 42 `.map_err( / Line 42 maps every inner failure to provider errors, but timeout/transport distinction is a robustness/classification issue rather than a demonstrated critical failure. |
| E1-01 | E1 | REFUTED（P1） | Line 627 commit failure aborts migration; plugin stays disabled because live config unchanged, line 625 clone unmodified. / Lines 627-631 retain `.disabled` after save failure; discovery must honor that marker, so the excerpt does not prove continued admission. |
| E1-02 | E1 | REFUTED（P1） | Line 1318 comment documents that stale key reads None (fail-closed), never exposes new tools to wrong scope. / Lines 1318-1321 explicitly state the intermediate state fails closed as `None`; stale old keys cannot expose new Project B metadata. |
| E2-02 | E2 | DOWNGRADE（P2） | Line 17-27 shows registration captured before lock, allowing mount disposer (lines 120-121 referenced) to interleave with manual start, violating lifecycle serialization / Line 17 captures registration before line 27 awaits the service-manager lock, permitting stale lifecycle operations; impact is c |
| E3-03 | E3 | DOWNGRADE（P1） | Line 127 applies sanitize_segment which maps distinct IDs to same bucket; line 211 deletes excess without session isolation. / Line 127 derives storage from lossy sanitization, enabling cross-session spill deletion; impact is temporary recovery-file loss, not persistent data loss. |
| E5-02 | E5 | DOWNGRADE（P1） | Lines 72-82 check metadata.len() then call read_to_string which can block on FIFO or grow beyond cap on TOCTOU / Lines 72–82 rely on metadata length and `read_to_string`; FIFO blocking and growth are reachable, but requires an authorized special or concurrently changing file. |
| E6-05 | E6 | DOWNGRADE（P1） | Line 116 reuses existing .git without verifying owner_repo changed; line 117 pulls from stale origin. / `github_source.rs:116-118` pulls the existing `.git` cache without checking its origin, so changing the configured source can update from stale content; this is a logic bug, not P0. |
| G1-01 | G1 | DOWNGRADE（P0） | cache read at 1171, insert at 1212, invalidation at 1333 without generation fence / Lines 1171-1172 perform an asynchronous ownership read before later cache insertion, permitting invalidation/fill interleaving; authorization impact depends on further unscoped excerpt. |
| G4-02 | G4 | DOWNGRADE（P1） | Line 23’s probe is case-sensitive while the described uppercase `<MEMORY>` bypass is real, but this is a confidentiality-filter logic defect rather than proven security-grade exposure. |
| G4-08 | G4 | DOWNGRADE（P2） | Lines 24–29 swallow every deserialization error and install defaults, but malformed configuration being accepted is a robustness/configuration defect rather than P1 logic loss. |
| G4-09 | G4 | DOWNGRADE（P2） | Lines 201–215 explicitly skip a settling rewrite when `Done`, leaving the cursor potentially visible; this is a presentation robustness issue, not a core data-loss defect. |
| H1-06 | H1 | DOWNGRADE（P1） | Line 260 insert + 279 publish before 310 build validation; rejected request leaves Running. / Line 310 can fail after earlier run insertion/publication, leaving a stale entry, but the cited Simulated fallback scope limits impact. |
| H3-02 | H3 | DOWNGRADE（P1） | Line 500 clones preset without reading cfg.acp.adapters first; overwrites user config / Lines 499–505 prefer a preset before checking the user override, so saved customization can be overwritten; impact is configuration loss, not P1-level security failure. |
| H3-07 | H3 | DOWNGRADE（P1） | Line 1113 std::fs::read_dir on Tokio thread; lines 1125-1126 sync canonicalize in loop / Lines 1113–1125 perform synchronous directory and metadata operations, but the excerpt shows a robustness/performance issue rather than a proven P1 failure. |
| H5-17 | H5 | DOWNGRADE（P1） | L164 reports page length as total for unrestricted; misleading but not data-loss or logic break / Lines 160–165 return page length as unrestricted `total`, while scoped callers receive the full visible count. |
| H6-04 | H6 | DOWNGRADE（P1） | Lines 568-570 probe params.provider_type/base_url; line 581 writes to provider found by name only / Lines 567-580 probe request parameters, then mark the named saved provider verified without comparing configurations; real but depends on mismatched or concurrent inputs. |
| H6-05 | H6 | DOWNGRADE（P1） | Line 65 synchronous git2::Repository::clone in async handler; no spawn_blocking wrapper / Line 65 performs synchronous network cloning inside an async handler; worker starvation is plausible, but the excerpt does not establish runtime executor configuration or practical severity. |
| H6-07 | H6 | DOWNGRADE（P1） | Line 478 reads job schedule outside final lock scope; line 496 service.update_job uses stale schedule+new timezone / Lines 478 and 525 separate schedule reading from update locking, permitting a concurrent schedule overwrite; the race is real but narrow. |
| H6-08 | H6 | DOWNGRADE（P1） | Line 313 reads config; no file-lock acquisition; line 380 rename overwrites concurrent canonical saves / Lines 313-380 perform an unlocked whole-file read/write/rename with a fixed temporary path; concurrent lost updates are possible, though caller serialization may reduce reachability. |
| IF1-01 | IF1 | REFUTED（P1） | Lines 113-119 show `AccessController::new` is called but no `expect` panic in the excerpt; claim not proven / `AccessController::new` behavior is not shown; lines 113-119 only insert the synthetic controller and do not prove the claimed resolver panic. |
| IF1-03 | IF1 | DOWNGRADE（P1） | Line 205 shows `block_streaming` exists but no merge/usage visible; dead field not P1 / Lines 205 and 45-48 expose `block_streaming` but omit it from `ResolvedConfig`; the excerpt does not show it reaching delivery. |
| IF1-07 | IF1 | DOWNGRADE（P1） | Lines 259-262 return `true` for AdminOnly without checking admin; comment admits caller should check / Lines 259-262 say the caller checks admin status; the excerpt does not prove an unauthorized caller reaches this helper. |
| IF1-08 | IF1 | REFUTED（P1） | tie-break(88apiclaude/claude-opus-5-5): Excerpt shows diagnostic `getUpdates` request but no offset advance code visible in lines 107-123. Evidence of actual offset consumption missing. |
| IF1-15 | IF1 | DOWNGRADE（P1） | Lines 102-115 send event then immediately call deliver_run_media without finalize ack / Lines 103-115 send `RunComplete` then immediately deliver media; the comment asserts ordering “in practice,” but no completion acknowledgement is shown. |
| IF1-19 | IF1 | DOWNGRADE（P2） | Line 92 sets last_update before send at 94; failure leaves throttle consumed with no message / Lines 90-94 set `last_update` before the first send and propagate failure, so the defect is a delayed retry rather than data loss. |
| IF2-03 | IF2 | DOWNGRADE（P0） | Line 243 subtracts marker from budget after body already split; consumption precedes truncation / Line 250 truncates the already-consumed body, confirming silent loss; however no production send-path use of this helper is shown. |
| IF2-04 | IF2 | DOWNGRADE（P1） | Line 757 only sends to queue; no interaction.create_response or defer call in Command branch / The shown slash path only queues the interaction without an ACK, but upstream acknowledgement may occur outside this excerpt; a protocol failure is not proven. |
| IF2-08 | IF2 | DOWNGRADE（P1） | Line 60 uses .find, returns first match; duplicate slug yields single candidate, not Ambiguous / Line 62 uses `find`, selecting the first matching guild rather than detecting duplicate matches; wrong-target resolution is real but typically P2. |
| IF2-11 | IF2 | DOWNGRADE（P1） | Line 464 synchronous store.load before async select; line 344 append also blocks in finalize / Line 464 performs synchronous `store.load` inside the async driver; actual Tokio-worker starvation and severity depend on runtime scheduling not shown here. |
| IF2-13 | IF2 | DOWNGRADE（P2） | Line 120 fetches single page of 200; line 123 returns that page as complete list / Line 120 requests only one 200-item page despite the documented pagination intent; incompleteness is real, but severity depends on >200-guild deployments. |
| IF4-07 | IF4 | DOWNGRADE（P0） | Line 60 `dm_policy`, 64 `group_policy` config fields never passed to Core permission system / Lines 60–72 define policy and allowlists, but the excerpt does not prove they are ignored by the runtime permission path. |
| IF4-12 | IF4 | DOWNGRADE（P1） | Line 143 spawns webhook, line 154 only warns on failure, line 157+ returns Ok/Connected / Lines 143–154 spawn the listener and only warn on registration failure; however, the excerpt also states catch-up polling may remain active. |
| IF4-15 | IF4 | DOWNGRADE（P1） | Line 34 `sweep_stale` calls line 39 sync `read_dir` in async start/poll context / Lines 39–44 perform synchronous directory enumeration, but the excerpt does not show whether this runs on a runtime worker. |
| IF4-20 | IF4 | REFUTED（P1） | tie-break(88apiclaude/claude-opus-5-5): Excerpt shows `FlexPayload::new(to, alt_text, contents)` called at line 108, which likely constructs proper envelope. Missing context: FlexPayload struct definition and serialization logic required to assess wire format. |
| IF4-25 | IF4 | DOWNGRADE（P1） | Line 99 adapter check_mention, line 265 empty metadata; Core re-checks with different rules / Lines 98–100 perform an adapter mention check, but the excerpt does not prove Core independently rejects an accepted exact Matrix mention. |
| IF4-26 | IF4 | DOWNGRADE（P1） | Line 78 `deduper.try_insert` calls sync rusqlite in async loop; line 79+ `query_row` sync / Line 78 calls synchronous `deduper.try_insert` from an async loop, but the excerpt does not establish the underlying DB operation blocks this runtime. |
| IF4-32 | IF4 | DOWNGRADE（P1） | Line 210 `write_tx.read().await` holds guard during send; line 197 stop waits for write lock / Line 210 holds the read guard across the awaited send path, creating a plausible shutdown deadlock, but saturated writer behavior is not shown. |
| IF4-35 | IF4 | DOWNGRADE（P1） | Line 322 `.ok()` converts query failure to None, line 350+ uses handle.id as conversation / Lines 321–322 suppress chat lookup errors and lines 349–356 fall back to a private identity, but `is_group` fallback does not prove a group reply targets one contact. |
| IF4-16 | IF4 | DOWNGRADE（P2） | Line 489 `res.bytes()` unbounded buffer, no cap enforcement before memory allocation / Line 489 buffers the full response before any cap check, confirming unbounded memory, but disk usage is not shown exceeding the cap. |
| IF4-27 | IF4 | DOWNGRADE（P2） | Line 9 HashSet grows unbounded at runtime, line 23 prune only on startup, no eviction / Lines 6 and 9 call it LRU but use an unbounded `HashSet`; runtime pruning or DB growth is not shown in this excerpt. |
| IF4-28 | IF4 | DOWNGRADE（P2） | Line 72 `get_media_content` allocates full Vec, line 73 checks size after allocation / Line 72 downloads before checking `bytes.len()` at line 73, confirming post-download enforcement; outbound unbounded allocation is not shown. |
| IF5-06 | IF5 | DOWNGRADE（P1） | Line 156 `token: String` is copied at startup; GatewayContext never refreshes expired token / Lines 143-156 snapshot the initial token into `GatewayContext`; reconnect behavior is not shown, so expiry impact is unproven. |
| IF5-20 | IF5 | DOWNGRADE（P2） | Line 29 uses `Client::new()` with no timeout; line 165-168 awaits synchronously in socket loop / Lines 27-32 create a client without an overall timeout, but the excerpt does not establish that `users.info` can indefinitely withhold a response. |
| IF6-14 | IF6 | DOWNGRADE（P1） | Line 663: pinned.is_some() uses Utc::now(), else uses stale captured now from line 980 after authorizer wait / Line 663 uses captured `now` when unpinned, allowing expiry during authorization to pass the final admission check; the trigger is real but narrowly timed. |
| IF6-12 | IF6 | DOWNGRADE（P2） | Line 216-218: std::thread spawned with stdin.lock(); line 278-283: stop doesn't join or cancel reader thread / Lines 216–218 spawn a detached stdin reader that is not stopped or joined; restart can leave the old reader consuming input, though impact is robustness/resource behavior. |
| IF6-13 | IF6 | DOWNGRADE（P2） | Line 603: outcome:None inserted; line 1007: prune keeps outcome=None forever; cancel during authorizer await leaves unreachable / Lines 599–605 insert an operation with `outcome: None` before authorization; cancellation can strand it, but the cited public seam makes this a robustness/resource defect |
| EE3-02 | EE3 | DOWNGRADE（P1） | L184-202 pregrant awaits load_candidates + parse_skill_file synchronously on executor thread, no spawn_blocking wrapper present / Lines 184-187 and 202 perform synchronous filesystem work inside an async path, but this is a responsiveness defect rather than a demonstrated permission bypass. |
| GX1-07 | GX1 | DOWNGRADE（P1） | Lines 879, 1139, 1141 call synchronous store.touch_device on Tokio workers without spawn_blocking / Line 879 synchronously calls `touch_device`; this can block a worker, but the excerpt alone does not establish the broader severe connection-lifecycle impact. |
| GX2-03 | GX2 | REFUTED（P0） | The cited line 622 logs `send_err` from an *error-reply* send, not from `send_public_telecom`; the `ChannelPublicReplySink::send_reply` path correctly does `map( / Lines 744-750 explicitly avoid stringifying transport errors; the cited excerpt does not show a token-bearing error being logged. |
| GX2-02 | GX2 | REFUTED（P1） | tie-break(88apiclaude/claude-opus-5-5): Excerpt shows identical `channel_id:conversation_id` format at both sites; `:` in IDs or channel rename are speculative, unsupported by the code shown. |
| GX2-04 | GX2 | REFUTED（P1） | tie-break(88apiclaude/claude-opus-5-5): Excerpt shows no re-IDing of merged messages; dedup runs before admission; claimed replay window failure is speculative, not supported by shown code. |
| GX2-06 | GX2 | REFUTED（P2） | Excerpt lines 487-495 show eviction IDs are used inside the same DB transaction which is correctly rolled back on `TurnTooLarge`; in-memory `Eviction` struct is a local variable, not persisted or returned to callers outside that path. No real defect. / Lines 487-495 may accumulate eviction IDs, but  |
| GX2-07 | GX2 | REFUTED（P2） | The excerpt explicitly shows `take(66)` plus post-read `match bytes.as_slice()` catches all TOCTOU outcomes; the finding's own analysis concedes no security bypass is possible. Purely informational, not a defect. / Lines 101-113 revalidate the actual bytes after reading; concurrent growth or truncat |
| GX3-03 | GX3 | REFUTED（P1） | tie-break(88apiclaude/claude-opus-5-5): Excerpt starts mid-function at line 74; no lock/transaction is visible, so the TOCTOU race cannot be confirmed. Without the race, every n==0 case returns meta.state matching the DB. |
| GX3-05 | GX3 | REFUTED（P2） | Lines 425-428 interpolate from compile-time constant slice (408-424). No runtime injection vector; maintainability concern only, severity inflated. / Lines 425-427 interpolate only compile-time constants from the fixed `needed` slice; no attacker-controlled or currently reachable injection path exis |
| GX3-06 | GX3 | REFUTED（P2） | Line 681 increments after spawn; docs say "re-queued," log says "re-delivered." Log wording mismatch, not a defect. Semantics match implementation. / Line 681 counts successfully registered/requeued messages; the documented return semantics are re-queued count, not completed deliveries. |
| GX3-07 | GX3 | REFUTED（P3） | Lines 372-386 build SQL+params conditionally but both blocks mirror same conditionals (376-377, 379-380 match 384-386). Compiler can't verify but human-readable pairing is clear. / Lines 372-386 currently keep SQL placeholders and parameter order synchronized; this is hypothetical future-edit risk,  |
| GX4-02 | GX4 | DOWNGRADE（P1） | Line 310-311 blocking write_all under admission mutex (manager.rs:1009); close/stop must wait for same lock / Lines 310–313 perform blocking `write_all`, but this excerpt does not prove it executes under the manager’s global admission lock. |
| GX5-01 | GX5 | REFUTED（P1） | `std::env::var()` reads process memory (not I/O), no syscall block; tokio runtime unaffected / Line 103 performs an in-process environment lookup, not blocking filesystem or network I/O; no runtime-thread stall is demonstrated. |
| GX5-02 | GX5 | REFUTED（P2） | Link configs are operator-written YAML (line 32 comment), not attacker input; env var size is operator-controlled / Lines 89–94 define rejection of oversized expansions, while operator-controlled environment and configs are not attacker-controlled as claimed. |
| GX5-03 | GX5 | DOWNGRADE（P3） | TOCTOU is real (lines 142-151) but race window is narrow, ticket consumption prevents double-spend; robustness defect not logic bug / Lines 142–151 can misreport a concurrent collision as invalid-ticket, but the single-use ticket prevents duplicate issuance; impact is error clarity only. |
| GX6-11 | GX6 | DOWNGRADE（P2） | L429 spawn without cancel tie; actual risk needs confirmation in exec / agent.rs:429 spawns execution independently of the response stream; no disconnect path shown connects receiver closure to execution cancellation. |

驳回或降级的发现**同样保留**在本目录 `final/<BID>.md` 与 `adjudication/<BID>.*.adj.md` 中，
每条都带两族理由与终裁理由——包括被驳回的那些。

---

## 4. 修复

修复在 worktree `/home/zou/data/workspace/Aleph-wt-fix-extfx`（分支 `review/occams-ext-fx-fixes`）中进行：
按批并行的修复代理只编辑自己那一批的文件，**不运行 cargo**、不提交；
编排侧逐批提交，最后把分支合入 `main`（无 PR）。

### 4.1 每批修复结果

| 批次 | 模块 | 存活 P0/P1/P2/P3 | 笔记 FIXED | 修复提交 | 终审 FIXED/PARTIAL/NOT_FIXED/REGRESSION |
|---|---|---|---|---|---|
| E1 | src/extension — lifecycle, loader, template, mod.rs (plugin lifecycle state machine) | 0/1/0/0 | 1 | `a959ad796 (+1)` | 0/0/0/0 |
| E2 | src/extension — root services (plugin ops, visibility, state, watcher, service manager) | 0/1/1/0 | 2 | `67cd45c28` | 0/0/0/0 |
| E3 | src/extension — hooks executor + hook types (ordering, consent, idempotency) | 1/2/0/0 | 3 | `e28333b88` | 0/0/0/0 |
| E4 | src/extension — manifest parsing/validation + effects (schema, fail-closed, effect claims) | 1/4/1/0 | 6 | `8273cb7a8` | 0/0/0/0 |
| E5 | src/extension — WASM runtime + registry/registrar + mcp_config (sandbox, single source of truth) | 2/4/0/0 | 8 | `8e63703e7 (+1)` | 6/0/0/0 |
| E6 | src/extension — marketplace install + panel publication (supply chain, publish path) | 4/1/0/0 | 7 | `3172596b8 (+1)` | 0/0/0/0 |
| EE1 | src/gateway/execution_engine — part A (execute core, tool dispatch) | 1/2/1/0 | 3 | `4f8a84c29 (+1)` | 2/1/1/0 |
| EE2 | src/gateway/execution_engine — part B + run_loop (streaming, cancellation) | 0/1/1/0 | 2 | `a77cc6669 (+1)` | 0/0/0/0 |
| EE3 | src/gateway/execution_engine — tests + slash command body + skill pregrant + btw_promote | 1/1/1/1 | 4 | `d14bf8d30 (+1)` | 0/0/0/0 |
| F1 | src/fetch — fetch provider contract (SSRF, redaction, size limits) | 0/2/1/0 | 3 | `a814bc765 (+1)` | 2/1/0/0 |
| G1 | src/gateway — channel registry + resume coordinator + event visibility + delivery queue | 1/4/1/0 | 6 | `a485d32e3 (+1)` | 2/2/2/0 |
| G2 | src/gateway — session projector + visibility rules + i18n (single source of truth for state) | 1/0/2/0 | 3 | `c52cffe27` | 0/0/0/0 |
| G3 | src/gateway — root part A (event bus, agent env, middleware glue, formatting) | 0/1/2/0 | 3 | `f3d202d98` | 0/0/0/0 |
| G4 | src/gateway — root part B (misc root services) | 1/6/3/0 | 13 | `8abbe00a5 (+1)` | 0/0/0/0 |
| GX1 | src/gateway — server + control_plane + admin_api (IPC surface, R4/R6 boundary) | 4/3/4/0 | 8 | `bbddd3393` | 6/2/3/0 |
| GX2 | src/gateway — telecom + inbound_router (routing, R5 delivery, R4 pure I/O) | 0/1/1/1 | 3 | `864b2e094` | 0/0/0/0 |
| GX3 | src/gateway — session_store + session_manager + busy_queue (state ownership, backpressure) | 0/2/1/0 | 3 | `7a648bdca (+1)` | 1/1/1/1 |
| GX4 | src/gateway — pty + event_emitter + events (process IO, event fanout) | 1/5/6/0 | 10 | `dd7b431a2 (+1)` | 0/0/0/0 |
| GX5 | src/gateway — security + middleware + surface + link + message_assembly + pipeline | 0/0/0/1 | 1 | `18a3cb7a2` | 1/0/0/0 |
| GX6 | src/gateway — voice + openai_api + btw + mcp_face + agent_env + formatter | 2/13/2/0 | 17 | `6db4604e7 (+1)` | 0/0/0/0 |
| H1 | src/gateway/handlers — part A (agent/users/projects/message CRUD handlers) | 1/5/2/0 | 8 | `1b7fdfe25 (+1)` | 6/2/0/0 |
| H2 | src/gateway/handlers — part B (session/config/provider handlers) | 3/4/2/0 | 9 | `dbed8c496 (+1)` | 0/0/0/0 |
| H3 | src/gateway/handlers — part C (tool/memory/misc handlers) | 1/6/0/0 | 8 | `2f6e017cc` | 7/0/0/0 |
| H4 | src/gateway/handlers — part D (auxiliary handlers) | 2/4/1/0 | 7 | `409c5d735` | 0/0/0/0 |
| H5 | src/gateway/handlers — teams + session db_handlers + memory + providers | 2/15/1/0 | 18 | `a5e4505f1 (+1)` | 16/2/0/0 |
| H6 | src/gateway/handlers — graph, cron, plugins, generation_providers, search_config, extensions, security_config, config, auth | 2/6/2/0 | 10 | `598bbcb86 (+1)` | 8/2/0/0 |
| IF1 | src/gateway/interfaces — telegram (+streaming) channel adapter | 0/15/3/0 | 17 | `c68dc8d7c (+1)` | 15/2/3/0 |
| IF2 | src/gateway/interfaces — discord + voice_call adapters (incl. relay tests) | 3/8/2/0 | 12 | `e6a6d68d4 (+1)` | 0/0/0/0 |
| IF3 | src/gateway/interfaces — feishu + whatsapp adapters (runtime/policy/auth splits) | 4/14/3/0 | 21 | `7bbb5b63b (+1)` | 0/0/0/0 |
| IF4 | src/gateway/interfaces — imessage + matrix + line + xmpp adapters | 8/20/6/0 | 35 | `60c423948 (+1)` | 27/7/1/0 |
| IF5 | src/gateway/interfaces — slack + qq + wechat + nostr adapters | 4/13/5/0 | 22 | `6ac730d48 (+1)` | 20/1/1/0 |
| IF6 | src/gateway/interfaces — sms, irc, signal, email, mattermost, webhook + root | 5/7/4/0 | 16 | `8075de297 (+1)` | 9/4/2/1 |

终审合计：FIXED 128 · PARTIAL 27 · NOT_FIXED 14 · REGRESSION 2（终审只跑了区域拼接轨道涉及的 15 批；其余 17 批的复核见 `fixes/FIX-<BID>.review*.md`）。

### 4.2 修复提交清单

| 提交 | 说明 |
|---|---|
| `a814bc765` | fix(fetch): close adjudicated findings from the extension/fetch/gateway review (F1) |
| `a959ad796` | fix(extension): close adjudicated findings from the extension/fetch/gateway review (E1) |
| `67cd45c28` | fix(extension): close adjudicated findings from the extension/fetch/gateway review (E2) |
| `e28333b88` | fix(extension): close adjudicated findings from the extension/fetch/gateway review (E3) |
| `8273cb7a8` | fix(extension): close adjudicated findings from the extension/fetch/gateway review (E4) |
| `8e63703e7` | fix(extension): close adjudicated findings from the extension/fetch/gateway review (E5) |
| `3172596b8` | fix(extension): close adjudicated findings from the extension/fetch/gateway review (E6) |
| `a485d32e3` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (G1) |
| `c52cffe27` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (G2) |
| `f3d202d98` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (G3) |
| `8abbe00a5` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (G4) |
| `1b7fdfe25` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (H1) |
| `dbed8c496` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (H2) |
| `2f6e017cc` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (H3) |
| `409c5d735` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (H4) |
| `a5e4505f1` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (H5) |
| `598bbcb86` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (H6) |
| `c68dc8d7c` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (IF1) |
| `e6a6d68d4` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (IF2) |
| `7bbb5b63b` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (IF3) |
| `60c423948` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (IF4) |
| `6ac730d48` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (IF5) |
| `8075de297` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (IF6) |
| `4f8a84c29` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (EE1) |
| `a77cc6669` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (EE2) |
| `d14bf8d30` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (EE3) |
| `bbddd3393` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (GX1) |
| `864b2e094` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (GX2) |
| `7a648bdca` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (GX3) |
| `dd7b431a2` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (GX4) |
| `18a3cb7a2` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (GX5) |
| `6db4604e7` | fix(gateway): close adjudicated findings from the extension/fetch/gateway review (GX6) |
| `1d957e105` | fix(fetch): repair the review-round splice damage and the open fix-review findings (F1) |
| `d28f829a0` | fix(extension): repair the review-round splice damage and the open fix-review findings (E1) |
| `326b7bd25` | fix(extension): repair the review-round splice damage and the open fix-review findings (E5) |
| `e5b01ccc5` | fix(extension): repair the review-round splice damage and the open fix-review findings (E6) |
| `87495a19a` | fix(gateway): repair the review-round splice damage and the open fix-review findings (G1) |
| `16f71585b` | fix(gateway): repair the review-round splice damage and the open fix-review findings (G4) |
| `6972885e9` | fix(gateway): repair the review-round splice damage and the open fix-review findings (H1) |
| `bbb670606` | fix(gateway): repair the review-round splice damage and the open fix-review findings (H2) |
| `582ac96b7` | fix(gateway): repair the review-round splice damage and the open fix-review findings (H5) |
| `a125f7ecd` | fix(gateway): repair the review-round splice damage and the open fix-review findings (H6) |
| `4d227f37f` | fix(gateway): repair the review-round splice damage and the open fix-review findings (IF1) |
| `3d1b0397a` | fix(gateway): repair the review-round splice damage and the open fix-review findings (IF2) |
| `c4283badb` | fix(gateway): repair the review-round splice damage and the open fix-review findings (IF3) |
| `0ed06eb7b` | fix(gateway): repair the review-round splice damage and the open fix-review findings (IF4) |
| `41d86153d` | fix(gateway): repair the review-round splice damage and the open fix-review findings (IF5) |
| `69a5ba74d` | fix(gateway): repair the review-round splice damage and the open fix-review findings (IF6) |
| `2b77b86aa` | fix(gateway): repair the review-round splice damage and the open fix-review findings (EE1) |
| `b1e430f48` | fix(gateway): repair the review-round splice damage and the open fix-review findings (EE2) |
| `d6dde17e7` | fix(gateway): repair the review-round splice damage and the open fix-review findings (EE3) |
| `f8cde9556` | fix(gateway): repair the review-round splice damage and the open fix-review findings (GX3) |
| `017cfcf30` | fix(gateway): repair the review-round splice damage and the open fix-review findings (GX4) |
| `85b061844` | fix(gateway): repair the review-round splice damage and the open fix-review findings (GX6) |
| `c5fca8d0c` | fix(gateway): cross-batch repair of the review round splice damage |

### 4.3 三条修复轨道（本轮实际发生的事）

| 轨道 | 执行者 | 覆盖 | 说明 |
|---|---|---|---|
| A. 代理波次 | Agent 工具子代理（`fable` = MiniMax-M3.1-Flash 廉价档；`opus` = 88apigpt） | 32 批中的大部分 | 逐批编辑自己批次的文件，不跑 cargo、不提交；编排侧逐批提交 |
| B. 区域拼接 | 原始 magpie API + `kimi/kimi-for-coding` | A 轨因额度事故未覆盖的 9 批 + 复核发现的 14 条 | 模型重写 finding 所在 ±30 行区域，脚本按区域回填（**见 §5.5 的代价**） |
| C. 手写 | 编排侧（本人） | 编译错误、区域损坏、复核打回的条目 | 逐条对照 **基线提交** 修，不信模型 |

三轨产出都进同一个 worktree，最后统一走一次 `cargo check` 与一次跨族修复复核。

### 4.4 最终复核后返工的条目

跨族修复复核（推理档对 diff 逐条判定）在 15 批上打回 27 条 PARTIAL / 14 条 NOT_FIXED / 2 条 REGRESSION。
其中以下条目由编排侧按基线语义重写：

| 条目 | 问题 | 处理 |
|---|---|---|
| G1-01 | 缓存填充与失效并发：预 rescope 的 ownership 会在失效后写回缓存 | `OwnershipCache` 加 `generation`，填充前捕获、代际不符则重读一次并在重试后 fail-closed；`forget_session` 在同一把锁内推进代际 |
| GX3-06 | REGRESSION：为一条已被裁决驳回的发现改了日志措辞 | 回滚到基线原文（该文件对基线零 diff） |
| IF6-09 | REGRESSION：Signal 重试预算耗尽只体现在本地变量上，外层只看 `JoinHandle` 结果 → 看起来像正常断开 | 监控任务用 `Arc<AtomicBool>` 把耗尽状态带出，外层映射为 `ChannelStatus::Error` |
| IF5-09 | WeChat 运行时把平台类型 `"wechat"` 当成实例 ID 写进入站消息 | 运行时持有**注册时**的 `ChannelId`，通道构造时传入 |
| IF1-08 | `EditedMessageIsTooLong` 被当作良性错误 → 超长答案被静默截断 | 从良性集合移除；编辑超限时按纯文本切块（切在格式化之前，每块都是合法 HTML）作为新消息补发 |
| IF1-20 | 状态表情机只比较状态，同状态不同表情永不更新 | 早退条件加上表情比较 |
| IF1-01 | 审批能力只带第一个 bot 实例 → 多账号部署用错 bot 回应审批 | 审批能力携带全部账号实例与按账号的 access 表 |
| IF4-29 | MUC 自消息过滤拿 nick 比 bare JID，而 join 用的是 `config.nick` → 自己的房间消息被当成入站 | `convert_message` 增加 `muc_nick` 参数，用实际 join 的 nick 过滤 |
| H1-01 | `remove_user_hooks` 不持 `HOOKS_MUTEX` | 与 `append_user_hook` 同一把锁 |
| H1-04 | 非法 `active_hours` 被静默丢弃 | 返回 INVALID_PARAMS |
| H5-04 | `create_provider_inner` / `set_default_provider_inner` 先改共享 config 再保存 | 改成先改副本、保存成功后再替换共享 config |
| H6-02 | 搜索配置的删除先删 vault 密钥再存 config | 先存 config，失败则不动密钥 |
| H6-04 | 探测结果只按"类型+base_url"匹配就写回 `verified` | 新增 `saved_generation_probe`：类型、base URL、**凭据**三者都匹配才写回 |
| H6-08 / H6-09 | 安全配置整文件读改写没有加锁、权限跟随 umask | 五个写入函数统一走 `update_config_toml`：持 `<config>.lock` 咨询锁 → 解析 → 编辑（失败不落盘）→ `write_atomic`（0600 临时文件 + fsync + rename） |


修复原则（写进每个修复代理的提示词）：

- 最小改动，**保持可观察行为**：公共 API、错误变体、顺序 / 幂等 / 重试语义、序列化格式、
  `Send`/`Sync`、锁获取顺序与临界区边界——除非 finding 明确要求改变其中之一。
- 不动本批之外的文件；不动本轮不相关的模块。
- 每条新增的非平凡逻辑补一个小而聚焦的测试。
- 基线树里 20 个文件本身就是 rustfmt 脏的（`evidence/rustfmt-dirty-base.txt`）——
  **不做全树格式化**，只对本次触及且基线干净的文件单独 `rustfmt --edition 2021 --check`。

---

## 5. 验证

本轮只做**一次**统一验证（用户要求"全部模块 review 完成后统一 cargo check"），在 fix worktree 的分支状态上运行：

```
CARGO_TARGET_DIR=/home/zou/data/workspace/Aleph/target \
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
CARGO_BUILD_JOBS=1 \
cargo check -p alephcore --all-targets -j1
```

- `--all-targets` 覆盖 lib、bin、以及**全部测试目标**（本轮新增的单元测试全部在这里被编译）。
- 内存控制：`CARGO_BUILD_JOBS=1` + 关闭增量编译 + 关闭 debug info；这台机器 20G 内存、无 swap，
  `alephcore` 的 lib-test 目标 rustc 峰值约 18G，因此**同一时刻只允许一个编译**（`flock` 串行化）。
- 结果：见 §5.1。

### 5.1 统一 cargo check 的结果

| 项 | 结果 |
|---|---|
| lib | ✅ 通过（12 条既有 dead-code warning，与本轮无关） |
| lib test | ✅ 通过（新增测试全部编译通过） |
| 其他 target（bin / 集成测试） | ✅ 通过 |
| 退出码 | `CARGO_EXIT=0`（耗时 14m 48s，日志 `evidence/cargo-check-final.log`） |

合并进 `main` 之后在合并后的 `main` 尖上又跑了一次同一条命令：`CARGO_EXIT=0`（2.35 s——产物指纹
全部命中缓存：合并后的源码与 worktree 中已验证的源码逐字节一致，`main` 上其它会话的改动也已在
上一次统一检查中编译过）。**这一次没有重新编译任何东西**，所以它证明的是"合并没有引入新的源码状态"，
而不是"合并后的树又被完整编译了一遍"。

**新增测试只是被编译，从未被运行**（见 §7）。

### 5.2 rustfmt 闸门

`rustfmt --edition 2021 --check --config skip_children=true`（`skip_children` 避免格式化污染模块树）
对**本轮触及且基线干净**的 235 个文件逐个检查 → 0 个失败。
基线本身 rustfmt 脏的 9 个文件（`evidence/rustfmt-dirty-base.txt`）不做格式化，也不算失败。

### 5.3 diff 闸门

对全量 diff 的新增行做危险模式扫描：`unimplemented!` / `todo!` / `dbg!` / `println!` / `unsafe` /
`unwrap()` / `expect(` / `panic!` / `process::exit` / `Command::new` / `#[ignore]`。
命中项逐条人工确认（新增测试里的 `unwrap()` 是断言前置条件，不在生产路径）。

### 5.4 删除行审计（"这轮修复有没有顺手删掉别的东西"）

对全量 diff 的**被删除行**做确定性比对：凡是删除后在新文件里再也找不到的字符串都列出来（829 条候选，
其中 324 条落在被区域拼接碰过的 10 个批次）。人工复核的高风险调用点全部存活：
`channel_registry.rs::start_channel_inner`、`delivery_queue.rs` 的带 channel_id 守卫的 inflight UPDATE、
`execution_engine/execute.rs::admit_run`、`gate.rs::concurrency.acquire`。

### 5.5 区域拼接的代价（必须记录）

B 轨的脚本让模型重写一个 ±30 行区域再按区域回填。**它不解析 Rust**，因此：

- 9 个文件被写出**语法错误**（多余 `}`、缺失 `)`、字符串未闭合、整段代码被"带行号回显"）。
  全部由编排侧按基线语义手修，记录在 `evidence/splice-damage-repair.md`。
- 第一次统一 cargo check 报了 **35 个错误**（外加 lib-test 目标的 **61 个错误**，全部在新增测试里）。
  根因有两类：① 拼接删掉了原有代码（如 WeChat 的 `build_conversation_id` 被删、
  CLI 的 stdin 读取线程被删、Nostr 的 DM 构造被换成 `panic!`）② 新增测试臆造了不存在的 API
  （`admit_run` 的真实签名是 6 参数、`ToolInvocation` 的真实字段是 `{id, name, duration_ms, success, error}`、
  `RunState` 没有 `finished_at`）。
- 这些错误**在修复复核里看不见**：复核读的是 diff，不是编译结果。
  → 所以最后一道闸必须是 `cargo check`，不是模型复核。

### 5.6 跨族修复复核（最后一道模型闸）

对 15 批（区域拼接碰过的批次）用 `kimi/kimi-for-coding` 对最终 diff 逐条判定：
**FIXED 128 · PARTIAL 27 · NOT_FIXED 14 · REGRESSION 2**（判定口径见 §2）。
⚠️ 该复核脚本的第一版取的是 `git diff`（工作区 vs HEAD），而 32 个修复提交**已经提交**，
于是它看到的是空 diff、把已修的条目判成 NOT_FIXED —— 修正为 `git diff 79dacfa33`（分支基线）后才有意义。
**复核只能定位问题，不能替代编译**（§5.5）。

---

## 6. 有意没做的事（以及为什么）


| # | 没做的事 | 为什么 |
|---|---|---|
| 1 | **未改 `AcpAdapterManager` / `src/acp/manager/harness_admin.rs`** | 超出本轮范围（只审 `src/extension` / `src/fetch` / `src/gateway`）。H3-02「缺失 preset 不可选中」因此只修了 `acp_config.rs` 内的回退选择逻辑，**根治需要改 `from_entries` / `update_harness`**，留给 acp 侧单独一轮 |
| 2 | **未给 `src/fetch` 加 SSRF DNS 固定** | provider 侧发起的请求无法由调用方固定 DNS（模块 doc 已写明）。这是架构边界，不是可修的 bug；只能靠"provider 自己不发私网地址"约束 |
| 3 | **未把 `fetch` provider 接进 `web_fetch`** | 现状是刻意如此（模块 doc 声明未接线），接线属于功能变更而非本轮 review 的修复范围 |
| 4 | **未修复被裁决驳回的发现** | 例如 F1-01 / F1-04（firecrawl / crawl4ai 的脱敏其实已经由 `send()` / 后端负责）、E1 的 2 条、E2 的 1 条、E3 的 1 条、G1 的 1 条、H1 的 1 条、H3 的 2 条、IF1 的 2 条——它们在 `final/<BID>.md` 里带理由记录在案 |
| 5 | **未修复廉价档路由为 DEFER 的 3 条** | G4-05（保留 epoch 需原子化）、H1-08（不要逐出活跃运行）、H2-09（登出后锁内不提交刷新）——都超出"最小改动"边界，需要另一次设计 |
| 6 | **未做全树 rustfmt** | 基线树本身有 20 个 rustfmt 脏文件（`evidence/rustfmt-dirty-base.txt`），全树格式化会把无关改动混进本轮 diff。只对**本次触及且基线干净**的文件单独 `rustfmt --edition 2021 --check` |
| 7 | **未动会话开始时暂存的 `src/exec/manager.rs`** | 超出本轮范围（src/exec），且那是一份会回退已提交修复的半成品；本轮开始后被其它会话撤销 |
| 8 | **没有写大而全的测试套件** | 规则是"每条新增非平凡逻辑补一个小而聚焦的测试"；本轮所有新增测试都是单点回归测试 |
| 9 | **未运行 `cargo test` / `cargo clippy`** | 用户明确要求"无需 cargo check，直接提交，全部模块 review 完成后统一 cargo check"。本轮只做一次统一 `cargo check`（见 §5）；新增测试**只被编译，没有被运行** |
| 10 | **未做运行时/真机验证** | 通道适配器（telegram / discord / feishu / whatsapp / imessage / matrix / line / xmpp / slack / qq / wechat / nostr / sms / irc / signal / email / mattermost / webhook）的修复**没有真实流量验证**，只做了静态复核 |
| 11 | **没有做并发压力验证** | 锁、竞态、取消语义类发现只靠阅读与代码复核判定，没有 loom / tsan / 压测 |
| 12 | **没有重审本轮三个模块以外的代码** | 本轮范围就是 `src/extension` / `src/fetch` / `src/gateway`（822 文件 / 413.5K LOC） |
| 13 | **最终复核打回的条目没有全部修完** | 跨族复核一共给出 **54 条** PARTIAL / NOT_FIXED / REGRESSION；按合并后 `main` 的实际代码逐条重新核对后，仍成立 **38 条**，已整理成 [`ISSUES.md`](ISSUES.md)（现象 → 位置 → 当前状态 → 为什么没修 → 建议改法 → 验收标准）。其中需要新机制（任务句柄注册表 / 槽位预留 / 存储层可见性下推 / 取消安全结算）的条目留给下一轮。**本轮引入并已修复的回归**：`src/gateway/channel_registry.rs` 的 `start_channel` 丢掉了 `finish_legacy_operation` 调用 → 通道启动后永久 Busy（G1-03），已按基线语义补回 |
| 14 | **Telegram 审批能力的授权仍用第一个账号的 allowlist** | IF1-01 修到"用哪个 bot 回审批"✓；但 `authorize_actor` 只拿到 actor、拿不到会话上下文，无法定位账号 → 多账号部署下审批授权仍是第一个账号的口径 |
| 15 | **Telegram 超长答案补发时预览消息保留** | IF1-08 的补发是"追加新消息"，已存在的 preview 仍在 → 用户会看到 preview + 完整答案的重复内容；改成"删掉 preview 再发"需要新的删除语义，未做 |
| 16 | **没有把区域拼接工具的产物逐行与基线对齐** | 区域损坏是**按基线语义手修**的（对照基线提交 `79dacfa33`），不是逐行机器比对；少数"模型顺手改了但 finding 没要求"的改动可能仍然留在 diff 里 |

---

## 7. 本轮没有验证的部分


- **新增单元测试未运行**：`cargo check --all-targets` 只证明它们能编译；**没有任何一条被断言过**。
- **未跑 `cargo clippy`**：新增代码的 lint 面（ needless clone、单测里的 `unwrap`、冗余分支）未检查。
- **未构建 Panel / WASM 侧**：`src/extension` 涉及面板发布路径，但 Leptos/WASM crate 未构建。
- **未做真机 QA**：§6 第 10 条列出的全部通道均无真实收发验证；本轮所有"修复完成"的结论都建立在
  **静态复核 + 推理档 diff 复核**之上，不等于运行时行为已验证。
- **修复复核是模型判断**：`fixes/FIX-<BID>.review.md` 由推理档模型对 diff 逐条判定，
  是第二道闸而不是证明；`PARTIAL` / `REGRESSION` 的判定可能有误。
- **裁决覆盖面**：跨族裁决对每批都跑了，但部分 P2/P3 只裁了 P0/P1 之后按同族判据外推。
- **统一 `cargo check` 跑在 fix worktree 的分支状态上**，不是合并后、其它会话最新提交之后的 `main` 尖。
- **候选表来自 64k 重扫**：廉价档首轮 24k 上限的扫描全部失败（thinking 吃满预算、无正文输出），
  `candidates/` 是 64k 重扫的结果；**24k 失败轮没有留下任何结论**，也没有参与分级。
- **最终复核只覆盖 15/32 批**：跨族修复复核只跑了被区域拼接碰过的批次（E5/EE1/F1/G1/GX1/GX3/GX5/
  H1/H3/H5/H6/IF1/IF4/IF5/IF6）；其余 17 批只有第一轮复核（`fixes/FIX-<BID>.review*.md`），
  那些批次的 diff 没有被第二个模型独立复核过。
- **区域拼接的损坏是手修的**：9 个语法错误文件、35 个 lib 错误、61 个 lib-test 错误都是编排侧对照基线
  逐个改的；没有工具证明"当前 diff 与基线的差异全部是有意为之"。
- **合并后的复核是单一模型族**：合并后 main 的 54 条复核条目由两个只读 agent（廉价档 / MiniMax-M3.1-Flash）逐条读代码判定——不是跨族共识，个别条目可能有误判；清单里每条都附了可自行复核的文件与行号。
- **只做了一次 cargo check**：本轮没有跑 `cargo test` / `cargo clippy`，也没有在合并后的 `main` 尖上
  再跑一次（合并后的尖由其它会话并发推进，随时在变）。

