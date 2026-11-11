# Fix brief — EE3

- worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx
- branch: review/occams-ext-fx-fixes
- findings file: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/findings/EE3.md
- final verdicts: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/final/EE3.md
- classification: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/classification/EE3.class.md
- fix note path (in worktree): docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/fixes/FIX-EE3.md

## FIX THESE (4)

### EE3-01 [P0] Pregrant checks the skill directory but follows an untrusted SKILL.md symlink
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `src/gateway/execution_engine/slash_skill_pregrant/mod.rs:188-204,216-227` (`pregrant`, `operator_owned`)
- **Failure mode**: 已有两个用户级根中的同名 skill：低优先级的正常文件为 catalog 注册了允许 `bash` 的行，高优先级目录的 `SKILL.md` 则是指向项目内文件的符号链接。扫描器跳过该链接，但 `ReadSkillTool::load_candidates` 仍把其目录列为第一候选。这里仅 canonicalize/检查目录，所以两个目录均被判作 operator-owned；随后 `parse_skill_file(loaded.join("SKILL.md"), ...)` 跟随链接，按项目文件的声明预授权。模型能够经正常项目文件编辑改变该声明，且无需编辑受保护的用户根。例如注册上界含 `bash`、链接目标当前声明 `[]` 时，模型把目标改为 `[bash]`，下一次人类输入该 `/skill` 就获得本不应由可写项目文件授予的免确认权限。实际 `skill_read` 又会以 `file_name escapes the skill directory` 拒绝同一文件，预授权却已经生效。这是有前置安装形状的边界漏洞，不是声称项目自身能够创建用户根下的链接。
- **Evidence**:
  > `let loaded = candidates.first()?;`
  > `crate::skill::parse_skill_file(loaded.join("SKILL.md"), crate::skill::guess_source(loaded))`
  > `let canonical = skill_dir`

  跨模块核对：`src/builtin_tools/skill_reader/read.rs:191-196` 只检查 `SKILL.md.exists()`，但 `src/builtin_tools/skill_reader/read.rs:364-370` 在真正读取时 canonicalize 文件并拒绝越出目录的目标；`src/skill/mod.rs:895-907` 的扫描会跳过符号链接文件，所以需由另一正常同名注册提供 catalog 行；`src/skill/manifest.rs:358-369` 的解析器直接打开文件并跟随链接。`src/gateway/execution_engine/slash_skill_scope.rs:174-180` 的注册上界确实仍生效，但不能代替对当前声明文件的信任检查。
- **Suggested fix**: 在接受每个候选之前也解析其 `SKILL.md` 的真实路径，采用 `skill_read` 相同的目录 containment 判据并确认真实声明文件受文件工具 denylist 保护；无法解析或越界则不预授权。保留既有根优先级、目录级去重、operator/typed/unattended 门槛及注册列表交集，不要改为跟随其他候选来绕过拒绝。
- **Confidence**: 0.95

### EE3-02 [P1] Every typed operator skill can synchronously scan its bundle on a Tokio worker
- VERDICT: DOWNGRADE | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/execution_engine/slash_skill_pregrant/mod.rs:179-204` (`pregrant`)
- **Failure mode**: 人类在正常请求中调用用户级 skill，且该 skill 含大量 references/scripts 文件或位于慢速文件系统 → `split().await` 在执行引擎的 async 任务中直接运行同步候选查找、canonicalize 和完整文件解析。`parse_skill_file` 不只是读取少量 frontmatter，还递归扫描 companion 文件并逐个读取/内容审计；这些同步操作间没有 yield，阻塞该 Tokio worker，延迟同 worker 上的其他请求、取消和定时器。即使该 skill 没有可预授权的工具，也照样完成整个扫描才返回。这不是测试夹具中的同步 I/O。
- **Evidence**:
  > `ReadSkillTool::load_candidates(project.as_deref(), &id)`
  > `if let Err(origin) = operator_owned(dir) {`
  > `crate::skill::parse_skill_file(loaded.join("SKILL.md"), crate::skill::guess_source(loaded))`

  跨模块核对：`src/skill/manifest.rs:410-411` 调用 `scan_companion_files`，其 `src/skill/manifest.rs:248-303` 使用同步 `std::fs::read_dir`、`metadata`、`read` 遍历和读取 companion 文件。`src/gateway/execution_engine/execute.rs:766` 直接 await 此 split，路径上没有 blocking-pool 包装；邻近的 `src/skill/mod.rs:525-548` 已将相同类别的目录查找/完整解析放进 `spawn_blocking`。
- **Suggested fix**: 将这条预授权路径的同步文件系统工作放到 blocking pool，保留失败时不授权的行为。迁移时显式保留当前 agent/project 的候选搜索上下文：不要假设 Tokio task-local 的 agent id 会自动传播进 `spawn_blocking`。继续使用现有解析器及全部审计，不要通过删除 companion 扫描来解决阻塞。
- **Confidence**: 0.99

### EE3-03 [P2] Strategy assertions create and permanently abandon a new SQLite directory on every read
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `src/gateway/execution_engine/tests/loop_origin_race.rs:619-626` (`strategy_store`)
- **Failure mode**: 执行这一组 loop-origin 测试 → 每次 `seed_strategy` 和每次只读的 `has_strategy` 都先创建新临时目录、调用 `.keep()` 放弃清理、打开一个新数据库，再尝试安装进 install-once global。第一次之后新 store 不会被采用，但磁盘目录及数据库已创建且永不由 `TempDir` 清理；断言越多、重复跑测试越多，遗留文件越多，完全不承载测试状态。
- **Evidence**:
  > `crate::strategy::set_global_for_test(Arc::new(`
  > `crate::strategy::StrategyStore::open(&tempfile::tempdir().unwrap().keep().join("s.db"))`
  > `crate::strategy::global().expect("strategy global set")`

  跨模块核对：`src/strategy/mod.rs:175-178` 的 `set_global_for_test` 丢弃 `GLOBAL.install(store)` 的结果，已安装时不会替换；本文件 `src/gateway/execution_engine/tests/loop_origin_race.rs:629-647` 的 seed 和查询均会再次调用此辅助函数。
- **Suggested fix**: 已有 global 时直接复用，不为查询构造候选数据库；将首次安装用的临时目录纳入测试 fixture 的所有权/清理范围，而不是每次调用 `.keep()`。保留共享 global 与唯一 session key 的测试语义，不要让各测试误以为可以替换 install-once store。
- **Confidence**: 1.0

### EE3-04 [P3] The model-initiated-load invariant compares an unchanged local object with its own snapshot
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P3
- **Location**: `src/gateway/execution_engine/slash_skill_pregrant/tests/what_may_be_pregranted.rs:115-122` (`a_model_initiated_skill_load_grants_nothing`)
- **Failure mode**: 测试执行真实 `skill_read` 后，声称证明“模型自行加载 skill 不改变当前 turn 的 restriction/pregrant” → 该断言只能为真：`before` 来自局部 `t.request.metadata`，中间的 `ReadSkillTool::call_json` 没有收到 `t`、其 metadata 或其 gate 的引用，末尾再次读取的仍是完全未被修改的本地 map。因此这个比较没有观察 skill load 后的实际授权效果，却被当成安全回归证据；相关实现变化也不会使该比较失败。这里报告的是明确恒真的现有断言，不是泛指缺少测试。
- **Evidence**:
  > `let before = (t.restriction(), t.pregrant());`
  > `.call_json(json!({ "skill_id": SKILL }))`
  > `assert_eq!((t.restriction(), t.pregrant()), before);`
- **Suggested fix**: 删除这一身份比较；若要保留其声称的回归保证，在同一真实请求 gate/context 内调用真实 `skill_read`，再观察受保护工具仍被拒绝、限制后的工具列表仍未扩大。保留现有“非 slash turn 不预授权”的有效断言和正文读取断言，不要用另一个不可变 metadata 快照替代效果检查。
- **Confidence**: 1.0

## Clean files

以下批次文件已完整阅读，未发现达到本轮证据要求的独立问题；不代表已经过运行时验证。

- `src/gateway/execution_engine/slash_command_body/tests.rs`
- `src/gateway/execution_engine/tests/goal_origin_race.rs`
- `src/gateway/execution_engine/tests/goal_origin_effects.rs`
- `src/gateway/execution_engine/slash_command_body/mod.rs`
- `src/gateway/execution_engine/slash_skill_pregrant/tests/mod.rs`
- `src/gateway/execution_engine/btw_promote/tests.rs`
- `src/gateway/execution_engine/slash_skill_pregrant/tests/who_may_pregrant.rs`
- `src/gateway/execution_engine/slash_skill_pregrant/tests/grant_arm.rs`
- `src/gateway/execution_engine/slash_skill_pregrant/tests/scope_and_children.rs`
- `src/gateway/execution_engine/slash_skill_pregrant/tests/fire_sites.rs`
- `src/gateway/execution_engine/slash_skill_pregrant/tests/kind_and_key.rs`

未执行：源码修复、cargo/check/test/clippy/build/rustc、动态复现、git 写操作。未确认命令参数注入或 test-only helper 泄入生产的独立缺陷；字符串型 fire-site 测试的普遍脆弱性未单独算作发现。

