# Plugins → Aleph Hub 融合设计 (Plugins → Aleph Hub Convergence)

> **状态**: 设计已确认，待实现。
> **前序**: [MCP 融合](2026-06-23-mcp-hub-convergence-design.md) · [Skills 融合](2026-06-23-skills-hub-convergence-design.md)
> **范围**: primer-only **+ 一处外科 install-engine 修复**。官方 plugin 以 Aleph Hub 为单一发现 / 安装 / 已安装来源。

## 1. 背景与定位 (Context)

「所有 skill / plugin / mcp 都以 Aleph Hub 为准」三部曲的第三部。前两部已落地：

- **MCP**（双引擎问题）：内置 preset 引擎与 Extensions Hub 引擎并存 → 退役 preset 引擎，把 preset 投影进 `aleph-hub` 槽。
- **Skills**（非双引擎，纯增量）：skills 早已经 Hub `run_install`→`GitDir`→`install_git_skill` 安装；缺口仅是无冷启动 primer + 无 Official 身份 → 新增 primer 投影，**不动 reconcile / 无迁移 / 不改 Panel**。

**Plugins 与 skills 同型，非双引擎问题**：`run_install` 的 `GitDir` 分支对非 Skill kind **已经**走 plugin 安装路径（`marketplace.install_to_scope`，见 `src/hub/install.rs:173-194`）。Hub 与 `plugin.marketplace.*` RPC **共用同一 `MarketplaceManager`**，不存在 MCP 那种独立持久化的第二引擎。

唯一缺口同 skills：**官方 plugin 无冷启动 primer、无 Official 身份**。外加一处 plugins 独有的阻抗不匹配（见 §3）。

**结论：primer-only + 一处 install 路径修复**。无引擎退役、无迁移、不动 reconcile、不改 Panel。

## 2. 现状事实 (Established Facts)

| 事实 | 来源 | 含义 |
|------|------|------|
| 内置官方 marketplace = `aleph-official`，manifest 在 `plugins/.claude-plugin/marketplace.toml`，列 7 个 plugin（`name`/`source`/`description`/`version`） | `plugins/.claude-plugin/marketplace.toml`、`MarketplaceManifest` | 官方 plugin 枚举来源 = **marketplace manifest**（非 per-plugin manifest，故"无 manifest.json"） |
| `BUNDLED_PLUGINS = include_dir!("$CARGO_MANIFEST_DIR/plugins")`，boot 抽取到 `~/.aleph/plugins/cache/aleph-official/` | `src/bundled/mod.rs:22`、`extractor.rs:207` | 编译期嵌入；marketplace 缓存按 marketplace 名分槽 |
| `OFFICIAL_PLUGINS_REPO = "https://github.com/rootazero/Aleph-plugins"` | `src/bundled/mod.rs:35` | 溯源 URL |
| `run_install` GitDir + 非 Skill → `marketplace.install_to_scope(entry.name, marketplace_name=source_id≠"local", User, None)` | `src/hub/install.rs:173-194` | **Hub 已能装 plugin**；安装按 `name` + `source_id`（当 marketplace 名）从本地缓存读取，**不消费 GitDir 的 git_url/subdir** |
| `mark_installed` 对 Plugin/Skill 按 `(kind, 大小写不敏感 name)` 折叠（MCP 才按派生 id） | `src/gateway/handlers/extensions/catalog.rs:63-92` | name 折叠 → primer 条目与活 `local:plugin` 同 name 折叠显 installed/Official |
| `plugin.toml` 的 `name` == marketplace 条目 `name`（实测 `diagnostics`/`phone-control`/`diff-viewer` 全一致）== 活 `PluginRecord.name` | `plugins/*/.claude-plugin/plugin.toml`、`reconcile.rs:plugin_to_entry` | name 折叠承重事实成立 |
| `upsert_entry` 把**条目自身的 `source_id` 字段**写入 `source_id` 列；`replace_source(slot, …)` = `clear_source(slot)` + 逐条 upsert | `src/hub/cache.rs:34-54,153-164` | 条目 `source_id` 必须 == 槽名 `aleph-hub`，否则远端 fetch 的 `replace_source` 清不掉它们 → 孤立 |
| 已有测试 `plugin_entry_marked_installed_by_name_case_insensitive` | `catalog.rs:223-240` | Plugin name 折叠机制已锁定，无需改 reconcile |
| 可复用 `parse_marketplace_toml_content(&str)` / `parse_marketplace_json_content(&str)` | `src/extension/marketplace/manifest.rs:25,30` | primer 可从嵌入 Dir 读 manifest 字符串后解析 |
| Settings▸Plugins（`views/settings/plugins.rs`）= 纯管理（`plugins.list` / URL 安装 / `plugins.uninstall` / auto-update），**无官方推荐区块** | panel 审计 | 无 MCP D6 对应物，不改 Panel |

## 3. 核心问题：install 路径阻抗不匹配 (The One Divergence)

primer 条目须 `source_id = "aleph-hub"`（§2 末行，槽正确性）。但 `run_install` 的 plugin 分支把 `source_id` 当作 **marketplace 名**传给 `install_to_scope`：

```rust
let marketplace_name = (ctx.entry.source_id != "local").then_some(ctx.entry.source_id.as_str());
// = Some("aleph-hub")  → search_plugin(name).retain(|r| r.marketplace_name == "aleph-hub")
//                       → 内置 marketplace 名是 "aleph-official" → 0 结果 → "Plugin not found"
```

**决议（用户选定 Option A）**：在 `run_install` 抽一个纯函数解析 marketplace 名：

```rust
/// Resolve which marketplace an install entry's plugin lives in.
/// Hub-official plugins (source_id == ALEPH_HUB_ID) come from the builtin
/// marketplace; "local" means "search all"; anything else is taken verbatim.
fn plugin_marketplace_name(source_id: &str) -> Option<&str> {
    match source_id {
        ALEPH_HUB_ID => Some(BUILTIN_MARKETPLACE_NAME), // "aleph-hub" → "aleph-official"
        "local" => None,
        other => Some(other),
    }
}
```

一行外科改动，语义诚实（Hub 官方 plugin = 内置 marketplace），条目仍正确留在单一 `aleph-hub` 槽，Hub 安装按钮可用。`install.rs` 属 `src/hub/`（非 `src/harness/`，R10 不适用）；纯解析、无认知/业务逻辑，无红线问题。

## 4. 设计决议 (Decisions)

- **D1 — 冷启动 primer 投影**：新 `src/hub/official_plugins.rs` 把内置 marketplace manifest 的每个 `MarketplacePluginEntry` 投影成 `ExtensionEntry`，由统一 primer 写入 `aleph-hub` 槽 iff 槽空。镜像 `official_skills.rs`。
- **D2 — 枚举自 marketplace manifest**：从 `BUNDLED_PLUGINS.get_file(".claude-plugin/marketplace.toml")` 读取并 `parse_marketplace_toml_content`（缺 toml 则尝试 `.json`）。非扫 per-plugin 目录（plugins 无 per-plugin manifest 契约）。
- **D3 — canonical id / name**：`id = "aleph-hub:<entry.name>"`；`name = entry.name`（== plugin.toml name == 活 PluginRecord.name，name 折叠成立）。plugin 无 frontmatter slug 派生（异于 skills 的 `SkillId`），marketplace 条目 `name` 既是显示名又是安装键。
- **D4 — 统一 primer 合成**：`hub::primer::prime_official_catalog_if_empty` 追加 `entries.extend(official_plugins::primer_entries())`，与 MCP + skills 合成**一次** `replace_source`。一行改动。
- **D5 — install 路径修复**：§3 的 `plugin_marketplace_name`。**这是与 skills 的唯一差异**（skills 是纯 primer-only，因 `install_git_skill` 从 spec 的 git_url/subdir 安装，与 source_id 无关）。
- **D6 — 不动 reconcile**：name 折叠已成立（`plugin_entry_marked_installed_by_name_case_insensitive` 已存在）。`plugin_to_entry` / reconcile 字节不动。
- **D7 — 无迁移**：官方 plugin 已在 boot 由 `extract_bundled_content` 抽取进 marketplace 缓存，无 id-keyed vault 绑定（异于 MCP 的 D9）。
- **D8 — 不退役 RPC、不改 Panel**：`plugin.marketplace.*`（marketplace 注册 add/remove/update）+ `plugins.*`（list / URL 安装 / uninstall）是非重叠管理操作，保留。统一 `views/extensions/` 已渲染 Plugin kind；Settings▸Plugins 纯管理，无推荐区块（无 MCP D6 对应物）。

### install_spec 形态

```rust
InstallSpec::GitDir {
    git_url: OFFICIAL_PLUGINS_REPO.to_string(),
    subdir: Some(<source 去 "./" 前缀>),  // 如 "diagnostics"
    git_ref: None,
    sha256: None,
}
```

GitDir 对 plugin 是**路由标记**（让 `run_install` 走 plugin 分支）+ 溯源。plugin 安装实际从本地 marketplace 缓存按 `name` 读取（含 marketplace 自身的 sha256 完整性校验），**git_url/subdir/sha256 不被 plugin 安装路径消费**（文档注明，异于 skills 的 subdir 是承重的）。`requires_config = spec.requires_config()` → GitDir → `false`。

## 5. 架构组件 (Components)

```
src/hub/official_plugins.rs   [新]  project_plugin(&MarketplacePluginEntry) + primer_entries()
src/hub/primer.rs             [改]  追加 official_plugins::primer_entries()（一行 + doc/log）
src/hub/install.rs            [改]  plugin_marketplace_name() 纯函数 + 替换 plugin 分支解析
src/hub/mod.rs                [改]  pub mod official_plugins;
```

### 数据流

```
boot → prime_official_catalog_if_empty(cache)
     → aleph-hub 槽空? → replace_source(aleph-hub, MCP ++ skills ++ plugins)
     → 官方 plugin 离线可浏览为 Official 卡片

用户点 Install → extensions.install → run_install
     → GitDir + Plugin kind → plugin_marketplace_name("aleph-hub")=Some("aleph-official")
     → install_to_scope(name, Some("aleph-official"), User, None)
     → 从 boot 已抽取的 marketplace 缓存装

extensions.catalog reconcile：primer 条目 name="diagnostics"
     与活 local:plugin name="diagnostics" 经 (kind,name) 折叠 → 显示 installed/Official
```

## 6. 跨仓契约 (§6 — 用户掌握，仓外)

远端 `hub.heyaleph.com` catalog 发官方 plugin 条目时：

- `id` = `aleph-hub:<plugin-name>`
- **`name` == plugin 加载名（plugin.toml `name`）** — skill 标记是 **name-based**，比 MCP 的 id-based 契约**更严**；name 不符则裂 installed 状态
- `source_id` == `"aleph-hub"`（槽正确性）

## 7. 风险与边界 (Risks)

- **R1 — Option A 未来边界**：`source_id → 内置 marketplace` 映射假设所有 `aleph-hub` plugin 条目来自内置 `aleph-official` marketplace。将来远端 catalog 若供**其他 marketplace** 的 plugin，须改为在条目显式携带 marketplace 名（ExtensionEntry 目前无此字段）。记为已知边界，本期不做。
- **R2 — 部署**：release/CI 须 `git submodule update --init` 使 `BUNDLED_PLUGINS` 在发布 binary 非空，否则官方 plugin 只现于 `extensions.installed` 活视图（graceful 降级，不报错）。同 skills。
- **R3 — name 契约脆弱性**：见 §6。bundled 满足（实测一致）；远端契约靠仓外纪律。

## 8. 测试策略 (submodule-independent)

`plugins/` 是 git submodule，dev/CI 可能为空 → `BUNDLED_PLUGINS` 编译期嵌空。所有单测用**合成 marketplace.toml 字符串**或手搭条目，不依赖真 bundle。

- **`official_plugins.rs`**：
  - `project_plugin_yields_official_aleph_hub_plugin_entry`：合成 `MarketplacePluginEntry` → 断言 `id="aleph-hub:diagnostics"` / `kind=Plugin` / `Official` / `source_id="aleph-hub"` / `name` 匹配 / `GitDir{git_url=OFFICIAL_PLUGINS_REPO, subdir}` / `!requires_config`。
  - `primer_entries_projects_synthetic_manifest`：合成 marketplace.toml 字符串 → `parse_marketplace_toml_content` → 投影 → 断言条目数与字段。（不经 `BUNDLED_PLUGINS`，测纯投影逻辑。）
  - `primer_entries_tolerates_absent_bundle`：调真 `primer_entries()`（bundle 可能空）→ 不 panic，返回的每条均 well-formed（Plugin/Official/`aleph-hub:` 前缀）。
- **`install.rs`**：
  - `plugin_marketplace_name_resolves_sources`：`"aleph-hub"→Some("aleph-official")` / `"local"→None` / `"custom"→Some("custom")`。纯函数，无 `MarketplaceManager` fixture。
- **`primer.rs`**：扩展现有——`plugins_extension_does_not_clobber_others`：`query(kind=Plugin)` 与 `query(kind=Mcp)` 数量互不影响（MCP catalog.json 仍是稳定锚，`aleph-hub:context7` 存在）。
- **`catalog.rs`**：Plugin name 折叠测试已存在，无需新增（可选：补一条镜像 skills 的文档性断言）。

## 9. 实现约束 (Constraints)

- 守红线 R3（核心轻量化，不引重库）/ R10（不动 harness）/ 单源设计（`aleph-hub` 单槽，无 peer 源、无 dedup）。
- 极度节制 cargo 调用：默认不跑全量；高风险合并至多一次 `cargo check --lib`（Task 改 bin 时一次 `--bin aleph-server`）。
- 英文 commit / 代码注释；文档中英双语。
- 测试过滤多个时须置于 `--` 之后：`cargo test -p alephcore --lib -- hub::official_plugins hub::primer hub::install`。
