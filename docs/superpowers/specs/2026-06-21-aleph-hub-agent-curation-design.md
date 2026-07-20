# Aleph Hub — Agent 策展 + 双速架构设计 (Agent-Driven Curation)

> 本文是对 [`2026-06-20-aleph-hub-design.md`](./2026-06-20-aleph-hub-design.md) 的**定向修订**。
> 只改「策展由谁做」与由此牵动的自动化(§8),其余设计(契约 schema、网站、install_spec
> 推断/验证、trust 分级、enrich、emit、dedup、源适配器)**全部沿用,不变**。

## 1. 背景与变更 (What changes & why)

原设计:确定性流水线在 GitHub Actions cron 中**无人值守**地跑完整链路,其中「策展(curate)」
环节调 **Anthropic API**(`ANTHROPIC_API_KEY`)产出 `name/kind/category/tags/descriptions/
install_spec hint/sec_notes`。

变更:**策展改由 Claude agent(我)直接做,不再调 Anthropic API**;同时我也介入「采集
(discovery)」。核心张力:**定时 cron 无法唤起交互式 agent 来写简介**,因此引入双速架构把
「确定性自动化」与「agent 策展」解耦。

变更的接缝极小:流水线本就依赖注入,Anthropic 只占 `adapters.ts::makeLlm()` 一个 port 实现。

## 2. 锁定决策 (Locked Decisions)

| # | 决策 | 选择 | 理由 |
|---|------|------|------|
| A1 | 自动化 × 策展共存 | **双速架构** | cron 只跑确定性部分;策展是我按需提交的 git 版本化数据,cron 下轮并入。自动化永不阻塞在 agent 上,彻底去掉 `ANTHROPIC_API_KEY`。 |
| A2 | 发现(discovery) | **混合:代码广撒网 + 我审筛补充** | 复用已有且免费的确定性源适配器(GitHub 搜索/ClawHub/Hermes)做廉价广撒网;我负责质量审筛、编辑精选、补充发现 + 全部策展。我同时在采集与整理两环。 |
| A3 | 未策展条目的可见性 | **未策展即排除** | 不展示 AI 占位文案。目录只含我逐条把关过的条目——正是「集中整理、逐字相同」的价值底线,也呼应 P-Provenance 开源署名铁律。 |
| A4 | 契约 schema | **一字不改** | `catalog.json` 的 manifest/entry schema 不动,Aleph 侧解析器**无需同步**。 |

## 3. 架构:`CurationStore` 取代 `LlmClient`

```
确定性 cron (无 LLM key)                      Agent 会话 (按需,我)
─────────────────────────                    ──────────────────────
sources(GitHub 搜索…) → dedup                 读 data/queue/to-curate.json
   每个候选 → CurationStore.get(full_name)        ↓ 审筛 + 补充发现
     命中 → 下游全部安全闸 → emit                 写 data/curation/<owner>__<repo>.json
     未命中 → 追加 to-curate.json                  ↓ git commit
emit / 刷新 stars·trend / 部署 / keepalive      下一轮 cron 自动并入产物
```

- **新增 port**(取代 `LlmClient`):
  ```ts
  // ports.ts —— curation comes from a committed store, not an API.
  interface CurationStore { get(fullName: string): CurationRecord | null; }
  ```
  `CurationRecord` 字段 = 现有 `LlmCurateOutput`(`name/kind/category/tags/description_{en,zh}/
  long_{en,zh}/install_spec hint/sec_note_{en,zh}`),保持下游兼容。
- **`curate.ts` 几乎不动**:把 `ports.llm.curate(input)` 换成 `ports.store.get(full_name)`;
  命中后**原有下游全保留**——zod 复校 → 本地重推 `install_spec`(LLM/我的 spec 仅作 hint)→
  语义验证(D11)→ 安全清洗(§4.6)。未命中返回 `null`,由 orchestrator 记入队列。
- **trust 分级、install_spec 推断/验证、enrich、emit、dedup、sources 完全不变**。安全性零损失:
  软编辑字段来自我,硬安全裁决仍是确定性代码。
- **依赖移除**:`@anthropic-ai/sdk` 从 `package.json` 移除;`makeLlm()` 删除;`makeAdapters()`
  改注入 `makeCurationStore()`(读 `data/curation/`)。

## 4. 数据模型 (Data Model)

```
data/curation/<owner>__<repo>.json   # 我提交的策展记录,每仓一文件(可审、可 diff、可 PR)
data/queue/to-curate.json            # cron 写:已发现但策展缺失的 repo + 元数据快照(stars/pushed_at/readme_hash)
```

- **每仓一文件**:符合「多个小文件 > 大文件」,diff 清晰,便于按条 review / 社区 PR 策展。
- **队列**:cron 每轮重算(发现 ∖ 已策展);记元数据快照,便于我会话里离线审筛而不必再抓。
- **键**:规范化 `full_name`(小写),与 dedup 键一致。文件名 `<owner>__<repo>`(`/`→`__`)。

## 5. 修订 §8 自动化 (Automation, 去 ANTHROPIC_API_KEY)

- **`pipeline.yml`**(`schedule` cron + `workflow_dispatch`):`npm ci` → `npm run pipeline`
  (确定性:发现 → dedup → 查 CurationStore → 刷新 stars·trend(ETag 条件请求)→ emit → 写队列)
  →(产物有变)`GH_PAT` commit `public/catalog.json` + `data/*.json` →
  `curl -X POST "$VERCEL_DEPLOY_HOOK"` → keepalive 心跳。并发锁防重叠。
- **Secrets 精简为三**:`GH_TOKEN`(抓取限额)、`GH_PAT`(commit 身份)、`VERCEL_DEPLOY_HOOK`(部署)。
  **不再需要 `ANTHROPIC_API_KEY`。**
- **保留**:D12 下限闸(`MIN_ENTRIES` / `MAX_DROP_PCT`)、keepalive 心跳(D14)、per-source 守卫。
- **指标调整**:`inference-yield`(emitted/deduped)→ **`curation-coverage`(已策展/已发现)**,
  并在队列里暴露「待策展积压」数,让我知道下次会话该补多少。
- **`ci.yml`** 不变:`tsc` + 单测 + zod 校验 committed `catalog.json` + `next build`。

## 6. 三步走落地 (The Three Steps, revised)

| 步骤 | 落地内容 |
|------|----------|
| **1. 自动化实施计划** | `pipeline.yml`/`ci.yml`/`vercel.json`(**无 LLM key**);`CurationStore` port + `curate.ts` 改造 + 队列写入;`curation-coverage` 指标 + 心跳;移除 `@anthropic-ai/sdk`。 |
| **2. 部署 Vercel** | 准备 `vercel.json`(`/catalog.json` 缓存头 + Content-Type)+ 验证 `next build`。**导入仓库需用户 Vercel 账号**(交互登录,我无法替登):提供 (a) 控制台导入步骤,或 (b) 用户本地 `vercel login` 后由我驱动 `vercel` CLI。验证 M1:`GET /<deploy>/catalog.json` → 200 + json + 过 zod。 |
| **3. 真实试跑** | 不需 `ANTHROPIC_API_KEY`。用户给 **`GH_TOKEN`**;我跑确定性发现 → 得候选队列 → **亲自策展首批 ~15–20 条真实条目**(写 `data/curation/*.json`)→ 产出真实 `catalog.json` 覆盖手写 fixtures → 过 zod + 契约。 |

## 7. 测试 (Testing)

- `CurationStore` fake 注入(沿用现有 fake 模式):覆盖命中 / 未命中 / 队列追加三路径。
- `curate.ts` 复用现有测试(下游安全闸不变);把 `llm` 替身换为 `store` 替身。
- 新增:`curation-coverage` 指标计算、`to-curate.json` 写入、未命中即排除(不入产物)的断言。
- 不变:`catalog.json` 过 zod、契约投影、emit 下限闸、dedup/trust/enrich/install_spec 既有测试。

## 8. 范围外 / 风险 (Out of Scope / Risks)

- **范围外**:Agent-in-CI(把我排进 cron 自动策展)——已否决(仍需 Claude 凭证、可审性差)。
  契约 schema 变更、分片/签名链——沿用原 §13 fast-follow。
- **风险**:
  - **策展积压延迟**:新 repo 在我策展前不展示(A3 默认)。缓解:队列暴露积压数;我按需批量补。
  - **首跑产物小**:目录初期只含已策展条目 → D12 下限闸阈值需按「策展规模」设(初期放低,
    随策展量提升),避免误触红 CI。
  - **Vercel 导入**:需用户账号交互,非我能独立完成(§6 步骤 2 已给两条路径)。
