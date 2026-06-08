# Panel Provider 行为一致性 + 强壮性加固 — 设计文档

- **日期**: 2026-06-08
- **范围**: WebChat Panel(Leptos/WASM, `interfaces/webchat/`)三套 provider UI 的行为统一
- **架构定位**: 纯 Panel(R4 Interface 纯 I/O),零业务逻辑改动;后端契约已正确,本次只让 Panel 忠实反映它
- **隔离方式**: worktree 隔离实施

---

## 1. 背景与问题

Panel 「设置」下有**三套相互独立**的 provider UI,行为各不相同:

| 系统 | 目录 | 默认语义 | 徽章文案 | 行渲染 | 密钥回显 |
|---|---|---|---|---|---|
| Chat/LLM | `views/settings/providers/` | `is_default: bool`(全局单默认) | `Default`/`Verified`/OAuth `Connected` | `ProviderRowCard` + OAuth 内联 | 后端**明文回显** vault key → 表单预填 |
| 生成类(图像/视频/语音/转写) | `views/settings/generation_providers/` | `is_default_for: Vec<GenerationType>`(按类型默认) | `Default`/`Active` | 全内联按钮 | 介于两者,待实现确认 |
| Embedding | `views/settings/embedding_providers/` | `is_active: bool`(全局单激活) | `Default`/`Verified` | 内联按钮 | 后端 `api_key=None` 不持久化,走 `api_key_env` → 表单**永远空** |

用户诉求:**填写 / 保存 / 测试 / 验证 / 设为默认 / 已验证·默认标签**的所有行为逻辑保持一致;**加强代码强壮性,不产生结果漂移**;**密钥回显要稳定**。

### 1.1 后端契约(已正确,不改)

经核查 `src/gateway/handlers/{providers,generation_providers,embedding_providers}`,三套后端**已完整实现**目标契约:

| 系统 | test 成功 → 持久 `verified=true` | update/改凭证 → 重置 `verified=false` | setDefault/setActive 强制 `verified==true` |
|---|:---:|:---:|:---:|
| chat | ✅ `handlers.rs:446` | ✅ `handlers.rs:156` | ✅ `handlers.rs:650` |
| generation | ✅ `handlers.rs:612` | ✅ `handlers.rs:337` | ✅ `handlers.rs:495` |
| embedding | ✅ `embedding_providers.rs:547` | ✅ `embedding_providers.rs:284` | ✅ `embedding_providers.rs:425` |

**结论**:漂移与不一致全部出在 Panel 侧——Panel 用乐观更新/缓存、各写一套渲染、密钥回显三种模型,没有忠实反映后端这套统一契约。

### 1.2 漂移根因(已定位)

- **hydrate Effect 依赖污染**(`providers/detail_panel.rs:53-110`):表单灌入 Effect 同时读 `selected` **和** `providers` 信号(line 82),任何 list 刷新都会重新灌表单 → 覆盖正在编辑的内容,尤其把回显的真 key 覆盖用户输入。
- **乐观本地态**:生成/Embedding 用 `on_reload()` callback,Chat 用重拉 list,刷新时机与粒度不一 → 测试成功后徽章不及时更新、保存后 verified 实际被后端清空但 UI 仍显旧值。
- **密钥回显三模型**:chat 明文预填、embedding 永空、generation 未定 → "打开时密钥框显示什么"和"保存空字段算什么"答案不一致,容易把真 key 冲掉。

---

## 2. 设计决策(已与用户确认)

1. **范围**:Chat + 生成 + Embedding 三套全部对齐。
2. **测试/验证关系**:测试成功 = 自动打「已验证」(持久化),改凭证自动清除。测试与验证是同一动作两面,**不设双按钮**。
3. **徽章**:统一为「已验证」+「默认」**可同时显示**;消灭 Verified/Connected/Active 三套说法。
4. **行渲染**:三套全部收敛到共享 `ProviderRowCard`。

---

## 3. 详细设计

### 3.1 总原则

- **纯 Panel,R4 干净**:不改任何业务逻辑。唯一可能的 `src/` 触碰是**只读 I/O 整形**(见 3.5),与用户逐处确认后才动。
- **服务器是唯一真相**:根治"结果漂移"。

### 3.2 反漂移(Single Source of Truth)

- 每个写操作(save / test / setDefault / delete / oauth login·logout)结束后,**统一重新拉取权威 `list` 并据此重渲染**;不再做乐观本地修改 `verified`/`default`。
- **修复 hydrate Effect 依赖 bug**:详情表单**只在「选中项标识(name/id)变化」时灌一次**,不再依赖 `providers`/`entry` 数据信号 → 后台 list 刷新不再冲掉编辑内容与密钥框。
- 异步动作在**点击时捕获 id/name**,await 回来后从服务器结果取真相,不修改缓存条目。
- 三套统一"in-flight 禁用"守卫(saving/testing/setting_default/deleting)。

### 3.3 测试 / 验证(契约镜像)

- Panel 把后端 `verified` 当唯一真相显示。
- 点「测试」→ 调对应 test RPC → 成功后**重拉 list**(后端已持久化 verified=true)→ 「已验证」徽章与绿点自动亮。
- 保存(update)成功 → 重拉 list → 后端已清 verified → 徽章自动灭(忠实反映"改了配置需重新验证")。
- 「设为默认」按钮在 `verified==false` 时**禁用 + 提示"请先测试通过再设为默认"**,镜像后端 gate,避免点击被服务端拒绝造成困惑。

### 3.4 徽章统一

- 新增**共享 badge helper**(渲染右侧徽章 slot):
  - `verified==true`(或 OAuth `connected==true`)→ 绿色「已验证」。
  - `is_default`/`is_active`/`is_default_for` 非空 → 主色「默认」。
  - 两者**可并存**(同一行可同时显示「默认」+「已验证」)。
  - 生成类按类型默认统一显「默认」,详情面板内文字说明具体是哪些类型(图像/视频/…)。
- 行卡左上 `RowDot::Verified` 绿点统一由 `verified`/connected 驱动。
- i18n:统一文案 key(如 `settings.providers.badge_verified` / `badge_default`),en/zh 同步,保持 key parity。

### 3.5 密钥回显稳定(核心)

统一为一套**可预测、不可破坏**的契约,三套完全一致:

- 编辑框**永不预填真密钥**。打开时:
  - 有 key(`has_api_key==true`)→ 空框 + 占位「已配置 · 留空保持不变」。
  - 无 key → 占位「未配置 · 请输入密钥」。
- 一致的「已配置 / 未配置」状态指示,由派生布尔 `has_api_key` 驱动。
- 保存时**脏追踪**:用户输入了非空才发 `api_key: Some(...)`;空 → `None` → 后端保留旧密钥(后端已支持,见 `providers/handlers.rs:142-153` "vault retains the old one")。三套同一逻辑。
- **可能的只读 `src/` 整形**(逐处与用户确认):
  - **chat 停止明文回显密钥**:`providers/handlers.rs` 的 `handle_list`/`handle_get` 改为不返回 `api_key` 明文,只返回已有的 `has_api_key`。既消除 hydrate 漂移又消除明文下发的安全隐患。
  - **generation / embedding 的 `list` 补 `has_api_key` 派生布尔**(若当前未返回)。embedding 的"已配置"判定需结合 `api_key_env`(环境变量名存在)与 vault 状态——实现时确认其真实数据源。
- 若用户不希望动 `src/`,退化方案:Panel 侧一律忽略回显的 key、改用 `has_api_key`(chat 已返回)驱动占位,chat 明文仍下发但 Panel 不展示——仍能稳定,但保留明文下发隐患。**首选改 `src/` 只读整形**。

### 3.6 行渲染收敛

- 生成 / Embedding / OAuth 订阅行**全部改用** `ProviderRowCard`(badge slot + RowDot + 选中/已配置态)。
- OAuth 行当前因尾部 chevron、`w-10 h-10` 图标尺寸而单独内联(见 `provider_row_card.rs:15-17` 注释)。方案:给 `ProviderRowCard` 增加**可选** props(尾部 slot / 图标尺寸变体)承载该差异,而非各写一份。
- 目标:徽章 / 选中态 / 点击逻辑一处定义,全局生效。

### 3.7 动作反馈统一

- 统一测试结果展示:成功(✓ + 延迟或 message)/ 失败(✗ + error),三套同一组件。
- 统一保存后行为:**重拉权威 list + 瞬时「已保存」提示(约 2s)**,三套一致(合并现有 chat 的重拉 与 generation/embedding 的 toast 两种模式)。

---

## 4. 涉及文件(预估 ~12)

**Panel 共享组件**
- `components/provider_row_card.rs` — 增加 OAuth 变体(可选尾部 slot / 尺寸)
- 新增共享 badge helper(可置于 `provider_row_card.rs` 或新建 `components/provider_badge.rs`)
- 可能新增统一 key-field 子组件(封装"留空保持不变"占位 + 脏追踪)

**Panel 三套 detail**
- `views/settings/providers/detail_panel.rs`
- `views/settings/generation_providers/detail_view.rs` + `preset_setup.rs` + `add_custom.rs`
- `views/settings/embedding_providers/detail_panel.rs` + `add_panel.rs`

**Panel 三套 list/mod 渲染**
- `views/settings/providers/list.rs`
- `views/settings/generation_providers/mod.rs`
- `views/settings/embedding_providers/mod.rs`

**Panel API wire 结构**(补 `has_api_key`)
- `api/providers.rs`(后端已返回,补 Panel 结构字段)
- `api/generation_providers.rs`
- `api/embedding.rs`

**i18n**
- en / zh 文案 key(统一徽章 + 密钥占位)

**可能的只读 `src/` 整形**(逐处确认)
- chat `handle_list`/`handle_get` 停止明文回显 key
- generation / embedding `list` 补 `has_api_key`

---

## 5. 验收标准

1. 三套 provider 行:填写 / 保存 / 测试 / 验证 / 设为默认 / 徽章 行为逻辑一致。
2. 「已验证」与「默认」徽章可同时显示;文案统一;绿点统一由 verified 驱动。
3. 测试成功后徽章无需手动刷新即亮;保存后 verified 状态忠实反映后端(清空)。
4. **密钥回显稳定**:打开任一已配置 provider,密钥框一致显示"已配置·留空保持不变";保存(不动密钥)**绝不冲掉**已存密钥;后台 list 刷新不覆盖正在输入的密钥。
5. 「设为默认」在未验证时禁用并提示。
6. 三套 list 行渲染统一走 `ProviderRowCard`。
7. `cargo check`(及 wasm 构建)通过;i18n en/zh key parity;`cargo clippy` 净。
8. 无 R1/R4 红线违反:Panel 不含业务逻辑;`src/` 改动仅限只读 I/O 整形。

---

## 6. 非目标(YAGNI)

- **不统一后端默认语义**:生成类按类型默认是真实业务差异,保留;Panel 只统一交互长相。
- **不改 test/verified/setDefault 的后端业务逻辑**(已正确)。
- **不做与本目标无关的重构**。
