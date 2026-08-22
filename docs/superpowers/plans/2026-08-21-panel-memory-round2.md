# Panel 记忆 Tab 第二轮 — 实施记录

Spec: [2026-08-21-panel-memory-round2-design.md](../specs/2026-08-21-panel-memory-round2-design.md)
分支：`worktree-panel-memory-deepen`（基 `34b9fbacc`）· 实施提交 `5fc906d56`

## 交付对照（spec §1 缺陷表）

| # | 处置 | 落点 |
|---|---|---|
| C1 curated 双缺席 | ✅ CONNECT | `src/gateway/handlers/memory_curated.rs`（3 handler + 9 测试）· Panel `memory/curated.rs` · facet 首位 |
| C2 write_decisions 收不到 | ✅ CONNECT | `TraceKind::WriteDecision` + `TraceResult.write_decisions` + `WriteDecisionRow`（客户端）· Curated ledger |
| C3 检索透视 | ✅ CONNECT | `memory/xray.rs`（`funnel_scale` 3 测试）· `MemoryConfigApi::retrieve_with_trace` 收敛 Settings 的裸 `rpc_call` |
| C4 修正队列零消费者 | ✅ CONNECT | `MemoryApi::list_corrections` + `memory/corrections.rs`，挂 Feedback facet |
| C5 `match_field` 零渲染 | ✅ CONNECT | `CompressedFact.match_field` + NoteCard chip |
| C6 笔记 offset 恒 0 | ✅ FIX | `loader::load_more_notes` + `merge_note_page`（4 测试）+ 载入/总数行 |
| C7 双抽屉不对称 | ✅ 重构 | `memory/note_links.rs`，galaxy 与 drawer 共用 |
| C8 `validFacts` | ✅ CUT | wire + Panel DTO + CLI 夹具 + 源码级守卫 |
| C9 `ai_output`/`window_title` + Q/A 死设计 | ✅ CUT | `MemoryEntry.content` 单列；卡片、导出、CLI 同步 |
| C10 三个死参数 | ✅ CUT | `SearchParams.window_title` / `ListFactsParams.include_invalid` / `ClearParams` |
| C11 `aleph memory clear` 必然失败 | ✅ CUT | 两 handler + 注册 + census + banner 行；CLI 本地解释、非零退出 |
| C12 `dreaming.list_insights` 缺占位 | ✅ FIX | `handlers/mod.rs` phase-1 占位 |
| C13 Raw 徽标搜索态不一致 | ✅ FIX | 徽标改读 `raws.total` |
| C14 星系截断提示 | ⚠️ **报告不成立** | badge 一直存在；真缺陷是硬编码英文，已随 C15 修 |
| C15 硬编码英文 | ✅ FIX | drawer 2 处 + phone 9 处 + galaxy badge（15 个新 key，en/zh 对称） |
| C16 死参数 `_default_agent_id` | ✅ CUT | `register_graph_handlers` |
| C17 mod.rs 拆层 | ✅（部分） | 四个新功能各自成文件（`curated.rs` 392 · `xray.rs` 243 · `corrections.rs` 114 · `note_links.rs` 96），`LoadMoreNotes` 归 `pager.rs`；`mod.rs` **910 → 980**：三处新编排的净增，不是重复代码。**没有**做到 spec 里写的「≤900 不再膨胀」——如实记下而不是继续凑数字 |

## 明确未做（本轮范围外，非遗忘）

- **漏斗不标注「为什么掉」**：`StageTrace` 只有 `{name, duration_ms, input_count, output_count}`，掉落数是 `input - output`。给每一级贴一个原因（「低于阈值」「去重」）会是 Panel 在**发明一套检索器并不具备的词汇**——MemOS 的 `RetrievalFunnel` 能那么做是因为它的后端逐项发了 `droppedByLlm` / `identifier rejected` 这些具名计数，Aleph 的没有。要做那种漏斗，先加服务端的具名计数，别在客户端猜。
- **`ScoreSnapshot`（每级的逐条分数）仍零渲染**：DTO 有、服务端在 traced 模式下填得出，但一屏漏斗要的是「掉了多少」不是「每条多少分」。第二个真消费者出现再接。
- **phone 端 Curated / ledger / 修正队列**：phone 记忆面保持轻量浏览定位（spec §4 已裁）。phone 的笔记窗口仍是单次 `NOTE_WINDOW`，累载只做在 wide 端。
- **`PhoneShell::back_label` 的 i18n**：类型是 `&'static str`，21 个手机屏全传英文字面量 —— 这是 crate 级的类型改动，不是记忆 tab 的修复。已在代码注释里写明，不是漏做。
- ~~**真机 QA**：本轮**零真机验证**~~ —— **已作废，见文末「真机 QA」一节**：curated 三动词、累载按钮、ledger 都在真浏览器里跑过并抓到两个缺陷。仍**未经真机验证**的是修正队列与检索透视（后者本夹具结构上答不了，见 `relocate_notes.py`）。

## 验证

| 命令 | 结果 |
|---|---|
| `cargo test -p alephcore --lib` | 16577 passed / **12 failed** —— 与基线 `34b9fbacc` **逐条相同**（另建 baseline worktree 实测比对，见下） |
| `cargo test -p aleph-panel --lib` | 1051 passed / 0 failed |
| `cargo test -p aleph-cli` | 194 passed / 0 failed |
| `cargo test -p alephcore --features test-helpers --test '*' --no-run` | 编译通过 |
| `cargo check --bin aleph-server` | 通过 |
| `cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release` | 通过（出厂形态） |
| `cargo clippy -p aleph-panel --all-targets` | 零警告 |
| `cargo clippy -p alephcore --all-targets` | 4 条警告，全在 `loop_graph/`（未触碰，先于本分支） |
| i18n census + locale 对称 | 通过（en/zh 键集相等） |

**基线比对方法**（值得记的部分）：`--lib` 在 HEAD 上本来就有 12 条红，所以「13 条红」这个读数单独看毫无信息。开了一个 `34b9fbacc` 的 baseline worktree 跑同一条命令，`comm` 出差集 —— **恰好一条是我的**（`two_members_share_one_room_memory_and_nobody_elses`，读的是我改名的 `user_input`，行本身正确）。改测试后差集为空。

**变异证明**：`an_empty_write_decision_target_lists_the_recent_ledger` 手工破坏过一次。第一次变异（删掉 store 侧 `filter(|f| !f.is_empty())`）**没有变红**——因为 `LIKE '%%'` 本就匹配全部，那是个语义 no-op；换成 handler 侧拒空 target 才红，并点名文件行号。

## 值得记住的判据（本轮新增/复现）

1. **一个 handler 若自己组合分区，它就是第二个真源**。curated 三 handler 一行组合都不做，把 base id 交给工具走的同一个函数 —— 于是「读写同一个文件」按构造成立。反面：`session_write_id` 非幂等，组合两次落到幽灵分区。
2. **拒绝形状要连 `limit` 一起对齐**。少对齐一个字段，「被拒」就能与「空的」区分开。
3. **census 守卫只认它被教过的注册形状**。`for (method, verb) in [...]` 的动态注册让扫描器失明，它当场报错。修法是**换成它认得的形状**（三处字面量 `.register("…")`），不是给守卫加豁免。
4. **一个「报告中的 gap」要先回代码确认**（C14 不成立）；一个「基线里的红」也要先确认是不是自己造的（差集比对）。
5. **变异没变红，先怀疑变异**（`LIKE '%%'`），不是先怀疑守卫。

---

# 真机 QA（2026-08-21，第二次会话）

> 上一节末尾写着「本轮**零真机验证**」。这一节把那句话作废：curated 三动词与累载按钮都在真浏览器里跑过了，**并且抓到了两个 16.5k 单测全绿时存在的缺陷**——其中一个是本轮自己接的线。

夹具：`qa/memory_curated/`（`run.sh` + `patch_config.py` + `seed.py` + `relocate_notes.py` + `probe.py`），已登记进 `qa/README.md`，`tests/qa_fixture_hygiene.rs` 三条守卫对它绿。

## 夹具形状（三个决定，每个都是被现实推翻一次之后定下的）

1. **种子走 `tools.invoke` 的 `remember` / `note_manage`，不写数据库**。两条主张都是「同一个 store 的两张脸要一致」，夹具自己写 store 就等于拿自己的布局假设当断言。
2. **必须配一个 provider**（指向一个没人监听的端口）。第一版配了零个，理由是「这两个工具都不调模型」——种子每一次调用都死在 `tools.invoke requires ToolRegistry (boot phase 2)`：**工具注册表只存在于 `register_agent_handlers` 的 real-execution 分支上，而选择那条分支的判据是「有没有 API key」**。「不需要模型」不等于「那张脸还在」。
3. **`[[agents.list]]` 不能补**：生成出来的 config 已经有一条，追加同 id 的第二条是硬 TOML 解析失败，daemon 在网关起来之前就死。

出账方式：每个检查点的 oracle **刻意不是被驱动的那个 RPC**——磁盘上的 `MEMORY.md`，和 `remember` 工具自己（对 Panel 刚写下的文本报 `duplicate`，只有当它解析到同一个 store 才可能）。

## 逐项结果（9/9 PASS）

| # | 断言 | 结果 |
|---|---|---|
| 1 | 热区记忆 facet 领头、徽标 3 = `remember` 工具写的三条 | ✅ |
| 2 | 预算 145/2200 · 7%；中文条目 **21 字符**（按字节会报 57） | ✅ |
| 3 | 编辑条目 2 → 列表由**服务端快照**重绘（45→68 字符，预算 145→168），toast「已保存」 | ✅ |
| 3-oracle | 磁盘上新文本在、旧文本不在；`remember{add:<新文本>}` 被判 **duplicate** ⇒ 工具与 Panel 是同一个 store | ✅ |
| 4 | 删除条目 3 → 徽标 3→2、预算 168→144 | ✅ |
| 4-oracle | 文件里没有了；工具**接受**重新添加（不只是文件里没有，工具视图里也没有） | ✅ |
| 5 | 展开「写入尝试」：4 行，最新在前，`duplicate` 原样渲染；`probe ledger` 同为 4 | ✅（**修复后**，见下） |
| 6 | 条目 1 改成 2517 字符 → 服务端拒绝，toast 逐字给出 `over budget: 2591/2200 chars; replace or remove first`，**列表不变、文件不变、台账不增** | ✅ |
| 7 | 全部笔记徽标 1000（库里 1040）·「已载入 1000 / 1040」·「加载更多」· 分页 1/20 | ✅ |
| 8 | 点「加载更多」→ 徽标 1040、分页 1/21、那一行整个消失（没有更多可载） | ✅ |
| 9 | 翻到第 21 页：40 行、下一页禁用。**第 21 页在点击前不存在** | ✅ |

第 8 项顺带证明了去重是对的：库里 1040 条，徽标恰好 1040 ⇒ 第二页没有一行与第一页重复（重复会被 `merge_note_page` 按 path 折掉，徽标就会小于 1040）。

## 抓到的两个缺陷

### D1（本轮自己的线，**已修**）：写入尝试台账在出厂装机上恒空

`remember` 经 `caller_memory_partition` 把决策行写在**合成分区**（`main__u-owner`），而 `memory.trace` 的 RPC 面拿 Panel 手上的 base id（`main`）直查——于是这个「唯一能回答『为什么没记住』」的面，在**每一个有东西可看的装机上**都回答「还没有记录到写入尝试」。零报错，四条种子行就躺在隔壁分区。

同一个缺陷的工具面**早就修过**，修它的那段注释逐字写着 "reading the bare persona answered 'there are none' for every scoped run" ——RPC 面从没被扫过。判据是仓里已有的那条：**一个动词有 N 个面时，"谁能看" 要在每个面用同一个推导**。

修法：`handle_trace` 收 `Option<Arc<MemoryContextProvider>>`，在**可见性闸之后**用 `resolve_storage_id` 组合（那正是 `get_or_load_curated_store` 用的同一个推导，所以台账属于它上面那份 entries 的 store）。守卫 `a_scoped_session_reads_the_ledger_its_own_writes_landed_in`：两个分区各埋一行，断言只拿到 alice 那行——把组合改回 `agent.clone()` 已实测 **RED 并点名 file:line**。

### D2（先于本分支，**未修**，本轮只做记录）：Panel 笔记面在出厂装机上读的是空分区

同一条根因的另一半，范围大得多：`note_manage` 写 `main__u-owner`，而 `memory.listFacts` / `memory.stats` / 图谱三个 handler 都直查 `main`。真机实测：`note_manage` 建了 1040 条笔记，`memory.listFacts(agent_id="main").total = 0`，星图写「还没有笔记——和智能体聊几句」，四张统计卡全 0。agent 下拉里**只有** `Main Agent`，没有任何界面能选到那个分区。

**为什么不顺手修**：已确立的答案是读侧走 `session_read_ids`（base ∪ 本分区，这是 prompt assembler 与旗舰 `memory_search` 用的那条），而那需要 backend 支持多分区 list/count，并且要把笔记身份从 `path` 改成 `(agent_id, path)`——同一个 path 在两个分区里可以同时存在，而 Panel 的 `<For key=path>` 会撞。那是一轮的工作量，不是 QA 尾巴上的顺手一改；在这里草率动 P1 分区语义，正是判据清单里「一条只写在散文里的裁定防不住下一个真诚的修复者」警告的那种改动。

**仓里已经有一条为它红着的守卫**：`every_memory_dispatch_arm_composes_the_partition` 点名 `recall_context (line 1594)` ——12 条基线红里的一条。D2 是同一族在网关面的未扫部分。

夹具因此有 `relocate_notes.py`：把真写者产出的语料**改键**到 Panel 读的分区，好让第 7-9 项能被测到；它的 docstring 就是这个缺陷的现场记录，并写明它**不动** FTS/向量行——所以本夹具的检索透视一律 0→0，那是夹具的代价，不是产品的结论。

## 复验

| 命令 | 结果 |
|---|---|
| `cargo test -p alephcore --lib` | 16579 passed / **12 failed**，与基线 `34b9fbacc` **同一组**（逐条比对；其中就有 D2 的那条守卫） |
| `cargo test -p aleph-panel --lib` | 1051 / 0 |
| `cargo test -p aleph-cli` | 194 / 0 |
| `cargo test -p alephcore --features test-helpers --test qa_fixture_hygiene` | 3 / 0（新夹具被它判过） |
| `cargo check --bin aleph-server` | 零警告 |
| `cargo clippy -p alephcore --all-targets` | 4 条，全在 `loop_graph/`（先于本分支） |

## 本轮新增判据

6. **「不需要模型」不等于「那张脸还在」**。`tools.invoke` 只在 real-execution 分支上有注册表，而选那条分支的判据是「有没有 API key」——一个不调模型的工具照样需要一个 provider 存在。
7. **out-of-band oracle 不能是被驱动的那个 handler**。「Panel 写进去了」由**磁盘**和**工具面**回答；拿同一个 handler 去问，只证明它自洽。
8. **一个真机夹具的每一步"绕开"都要写清它绕开了什么**。`relocate_notes.py` 让第 7-9 项可测，代价是检索面在这次运行里说不了话——不写下来，下一个人会把 0→0 读成产品缺陷。
