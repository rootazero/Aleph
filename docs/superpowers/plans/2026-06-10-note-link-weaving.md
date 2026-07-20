# Note 层记忆网络编织（Link Weaving）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **执行环境**：按用户要求，在隔离 git worktree 中执行（superpowers:using-git-worktrees）。基于本地 main（worktree add 后若发现落后本地 main，先 `git reset --hard main`）。

**Goal:** 让 note 出生即联网、存量孤岛被周期性编织进 wiki 链接网络（spec: `docs/superpowers/specs/2026-06-10-note-link-weaving-design.md`）。

**Architecture:** 三条战线：① ingest prompt 链接硬契约 + `enforce_link_contract` 和谐门（一次修复 LLM 调用，失败放行）；② `note_manage` create 后返回 `related_notes` 候选（FTS，best-effort）；③ 新 `NoteWeaveStage` dream stage（孤岛检测 SQL→hybrid 候选→LLM 选边→`append_to_note` 双向写链），注册在 Consolidate 管线 NoteDecay 之前。全程 LLM 决定链到谁（R7），harness 只校验 token 防幻觉（镜像 `RefTable` 先例）。

**Tech Stack:** Rust (alephcore)、insta 快照、RecordingMockProvider/MockProvider/MockEmbeddingProvider 测试桩、SQLite NoteStore。

**关键既有 API（全部已存在，不要新造）：**

| API | 位置 | 用途 |
|---|---|---|
| `RefTable::{from_related, token, resolve_links}` | `src/memory/notes/ingest/ref_table.rs` | `[P<n>]` token 防幻觉解析（resolve_links 现为私有，Task 2 升 pub(crate)） |
| `gather_related` / `RelatedPage` | `src/memory/notes/ingest/retrieve.rs:55` | ingest 相关页检索（已接线，不动） |
| `NoteStore::{get_outgoing_links, get_incoming_links, hybrid_search_notes, search_notes_fts}` | `src/memory/notes/store.rs` | 孤岛检测 / 候选检索 |
| `NoteIndexer::append_to_note(agent_id, path, new_facts, new_links)` | `src/memory/notes/indexer.rs:413` | 写链 + 重索引（`CompoundApplyTx::add_link` 同款原语） |
| `extract_json_robust` | `src/utils/json_extract.rs` | LLM JSON 容错提取 |
| `DreamStage` trait / `DreamContext` | `src/memory/dreaming/{stages/mod.rs:40, mod.rs:95}` | stage 协议；`ctx.load_content(path)` 懒加载正文 |

---

### Task 1: ingest prompt 链接硬契约

**Files:**
- Modify: `src/memory/notes/ingest/prompts.rs`（rule 6 改写 + 新增 `PROMPT_LINK_REPAIR`）
- Snapshot: `src/memory/notes/ingest/snapshots/*compound_plan_base_prompt.snap`（insta 自动管理）

- [ ] **Step 1: 改写 rule 6**

在 `prompts.rs` 中将 `PROMPT_COMPOUND_PLAN` 里的 rule 6 整段（从 `6. Prefer linking a new \`create\`` 到 `just to satisfy a linking preference. Later consolidation links seeds.`）替换为：

```text
6. LINKING IS MANDATORY when the "Related existing pages" section below is
   non-empty: every `create` MUST carry at least one `links[]` entry or
   `relations` edge pointing at a `[P<n>]` token from that section. A note
   with no links is an orphan island — orphans rot unrecallable and are
   archived early, defeating the wiki. Only use `[P<n>]` tokens that
   ACTUALLY APPEAR in that section — never invent a token number (an
   out-of-range token is discarded and can cost you the whole note). ONLY
   when the section is empty (sparse wiki, or retrieval degraded) may you
   create a SEED note with an empty `links` list — do NOT skip a durable
   fact just because there is nothing to link to. Additionally, when you
   notice two EXISTING related pages that should reference each other,
   emit a `link` op to connect them.
```

- [ ] **Step 2: 新增修复 prompt 常量**

在 `PROMPT_COMPOUND_PLAN` 定义之后追加：

```rust
/// Repair prompt for the link-contract harmony gate
/// (`enforce_link_contract`). Given the linkless `create` ops from a plan
/// plus the same related pages the planner saw, asks the LLM to either
/// supply `[P<n>]` links or explicitly declare the note isolated.
pub const PROMPT_LINK_REPAIR: &str = r#"You maintain an Aleph personal-memory wiki.
The following NEW notes are about to be written with NO links, even though
related pages exist. For EACH note, either pick 1-3 related pages it should
link to, or mark it isolated when truly nothing relates.

Rules:
- Only use `[P<n>]` tokens that appear in the "Related existing pages"
  section below — never invent a token number.
- Link a page only when a reader of one note would benefit from the other.
- Output valid JSON only, no prose, no markdown fences:

{"repairs": [{"note_index": 0, "links": ["[P2]"], "isolated": false}]}

`note_index` is the `[note <i>]` index shown for each new note below.
`links` must be empty when `isolated` is true.
"#;
```

- [ ] **Step 3: 跑 prompt 测试，快照预期失败**

Run: `cargo test -p alephcore --lib ingest::prompts`
Expected: `base_prompt_snapshot` FAIL（快照不匹配），`base_prompt_mentions_every_op_kind` PASS（rule 6 新文案保留了反引号 `link` 等 op 名）

- [ ] **Step 4: 更新快照后复跑**

Run: `INSTA_UPDATE=always cargo test -p alephcore --lib base_prompt_snapshot`，然后 `cargo test -p alephcore --lib ingest::prompts`
Expected: 全部 PASS

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/ingest/prompts.rs src/memory/notes/ingest/snapshots/
git commit -m "memory: make ingest link contract mandatory and add repair prompt"
```

---

### Task 2: `RefTable::resolve_links` 升 pub(crate)

**Files:**
- Modify: `src/memory/notes/ingest/ref_table.rs:121`

- [ ] **Step 1: 改可见性**

```rust
// 旧
    fn resolve_links(&self, links: &mut Vec<String>, stats: &mut ResolveStats) {
// 新
    pub(crate) fn resolve_links(&self, links: &mut Vec<String>, stats: &mut ResolveStats) {
```

- [ ] **Step 2: 验证编译**

Run: `cargo check -p alephcore`
Expected: 编译通过（与 Task 3 同一提交，此步可与 Task 3 合并验证）

---

### Task 3: `enforce_link_contract` 和谐门

**Files:**
- Modify: `src/memory/notes/ingest/ingestor.rs`（新方法 + 两处接线 + 单测）

- [ ] **Step 1: 写失败的单测**

在 `ingestor.rs` 文件末尾新增测试模块（与 `plan_tests` 平级）。注意 `RelatedPage` 全字段 pub，`RecordingMockProvider::new(resp)` 固定返回一条响应：

```rust
#[cfg(test)]
mod link_contract_tests {
    use super::*;
    use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
    use crate::memory::store::SqliteMemoryBackend;
    use crate::providers::recording_mock::RecordingMockProvider;

    fn related_page(path: &str) -> RelatedPage {
        RelatedPage {
            path: path.to_string(),
            title: path.to_string(),
            summary: "a related page".into(),
            content_preview: String::new(),
            tags: vec![],
            content_hash: "h".into(),
            score: 0.5,
        }
    }

    fn linkless_create(path: &str) -> PageOp {
        PageOp::Create {
            note_path: path.to_string(),
            title: "T".into(),
            summary: "S".into(),
            facts: vec!["f1".into()],
            links: vec![],
            tags: vec![],
            relations: vec![],
        }
    }

    fn mk_ingestor(
        canned: &str,
    ) -> (
        tempfile::TempDir,
        DefaultCompoundIngestor<SqliteMemoryBackend>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = Arc::new(NoteIndexer::new(dir.path().join("note"), backend.clone()));
        let ing = DefaultCompoundIngestor {
            store: backend,
            indexer,
            provider: Arc::new(RecordingMockProvider::new(canned.into())),
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
            embedding_manager: None,
            gate: None,
        };
        (dir, ing)
    }

    #[tokio::test]
    async fn repairs_linkless_create_with_valid_token() {
        let (_d, ing) = mk_ingestor(
            r#"{"repairs":[{"note_index":0,"links":["[P1]"],"isolated":false}]}"#,
        );
        let related = vec![related_page("learning/a"), related_page("learning/b")];
        let ops = ing
            .enforce_link_contract(vec![linkless_create("learning/new")], &related)
            .await;
        match &ops[0] {
            PageOp::Create { links, .. } => assert_eq!(links, &vec!["learning/b".to_string()]),
            _ => panic!("expected create"),
        }
    }

    #[tokio::test]
    async fn out_of_range_token_dropped_and_op_passes_through() {
        let (_d, ing) = mk_ingestor(
            r#"{"repairs":[{"note_index":0,"links":["[P9]"],"isolated":false}]}"#,
        );
        let related = vec![related_page("learning/a")];
        let ops = ing
            .enforce_link_contract(vec![linkless_create("learning/new")], &related)
            .await;
        match &ops[0] {
            PageOp::Create { links, .. } => assert!(links.is_empty()),
            _ => panic!("expected create"),
        }
    }

    #[tokio::test]
    async fn explicit_isolation_is_accepted() {
        let (_d, ing) = mk_ingestor(
            r#"{"repairs":[{"note_index":0,"links":[],"isolated":true}]}"#,
        );
        let related = vec![related_page("learning/a")];
        let ops = ing
            .enforce_link_contract(vec![linkless_create("learning/new")], &related)
            .await;
        match &ops[0] {
            PageOp::Create { links, .. } => assert!(links.is_empty()),
            _ => panic!("expected create"),
        }
    }

    #[tokio::test]
    async fn empty_related_skips_repair_entirely() {
        // Provider 返回的也是合法 repair JSON——若误触发 LLM 调用并应用，
        // links 就不再为空，断言会失败。
        let (_d, ing) = mk_ingestor(
            r#"{"repairs":[{"note_index":0,"links":["[P0]"],"isolated":false}]}"#,
        );
        let ops = ing
            .enforce_link_contract(vec![linkless_create("learning/new")], &[])
            .await;
        match &ops[0] {
            PageOp::Create { links, .. } => assert!(links.is_empty()),
            _ => panic!("expected create"),
        }
    }

    #[tokio::test]
    async fn malformed_llm_response_passes_through() {
        let (_d, ing) = mk_ingestor("not json at all");
        let related = vec![related_page("learning/a")];
        let ops = ing
            .enforce_link_contract(vec![linkless_create("learning/new")], &related)
            .await;
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            PageOp::Create { links, .. } => assert!(links.is_empty()),
            _ => panic!("expected create"),
        }
    }

    #[tokio::test]
    async fn already_linked_create_not_touched() {
        let (_d, ing) = mk_ingestor(
            r#"{"repairs":[{"note_index":0,"links":["[P0]"],"isolated":false}]}"#,
        );
        let related = vec![related_page("learning/a")];
        let mut op = linkless_create("learning/new");
        if let PageOp::Create { links, .. } = &mut op {
            links.push("learning/existing".into());
        }
        let ops = ing.enforce_link_contract(vec![op], &related).await;
        match &ops[0] {
            PageOp::Create { links, .. } => {
                assert_eq!(links, &vec!["learning/existing".to_string()])
            }
            _ => panic!("expected create"),
        }
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib link_contract_tests`
Expected: 编译 FAIL（`enforce_link_contract` 未定义）

- [ ] **Step 3: 实现 `enforce_link_contract`**

在 `ingestor.rs` 顶部 import 区补充（已有 `RefTable` import 的话只加 `ResolveStats` 与 `PROMPT_LINK_REPAIR`）：

```rust
use crate::memory::notes::ingest::prompts::PROMPT_LINK_REPAIR;
use crate::memory::notes::ingest::ref_table::ResolveStats;
```

在 `impl<S: NoteStore + ...> DefaultCompoundIngestor<S>` 块中（`dedup_redirect_creates` 之后）新增：

```rust
    /// Link-contract harmony gate. When the related set is non-empty, a
    /// `Create` with neither `links` nor `relations` violates the mandatory
    /// linking contract in `PROMPT_COMPOUND_PLAN` rule 6. One lightweight
    /// repair LLM call asks for `[P<n>]` links (anti-hallucination via
    /// `RefTable`) or an explicit `isolated` declaration. Repaired links
    /// merge back into the op; every failure degrades to pass-through —
    /// linking is an enhancement and must never block memory persistence.
    async fn enforce_link_contract(
        &self,
        ops: Vec<PageOp>,
        related: &[RelatedPage],
    ) -> Vec<PageOp> {
        if related.is_empty() {
            return ops;
        }
        let violating: Vec<usize> = ops
            .iter()
            .enumerate()
            .filter_map(|(i, op)| match op {
                PageOp::Create {
                    links, relations, ..
                } if links.is_empty() && relations.is_empty() => Some(i),
                _ => None,
            })
            .collect();
        if violating.is_empty() {
            return ops;
        }

        // Repair prompt input: the violating notes plus the same [P<n>]
        // table the planner saw.
        let mut user = String::from("## New notes with no links\n\n");
        for (slot, &i) in violating.iter().enumerate() {
            if let PageOp::Create {
                note_path,
                title,
                summary,
                facts,
                ..
            } = &ops[i]
            {
                user.push_str(&format!(
                    "[note {slot}] path={note_path} title={title}\nsummary: {summary}\nfacts:\n"
                ));
                for f in facts.iter().take(6) {
                    user.push_str(&format!("- {f}\n"));
                }
                user.push('\n');
            }
        }
        user.push_str("## Related existing pages\n\n");
        for (i, rp) in related.iter().enumerate() {
            user.push_str(&format!(
                "{} {} — {}\n",
                RefTable::token(i),
                rp.path,
                rp.summary
            ));
        }

        let msgs = [UnifiedMessage::user(&user)];
        let resp = match self
            .provider
            .process(RequestPayload::new(&msgs).with_system(Some(PROMPT_LINK_REPAIR)))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!("link contract repair LLM failed (pass-through): {e}");
                return ops;
            }
        };
        let Some(json) = extract_json_robust(&resp.text_content()) else {
            warn!("link contract repair: no JSON in response (pass-through)");
            return ops;
        };

        let refs = RefTable::from_related(related);
        let mut ops = ops;
        let repairs = json
            .get("repairs")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut repaired = 0usize;
        for rep in &repairs {
            let Some(slot) = rep.get("note_index").and_then(|v| v.as_u64()) else {
                continue;
            };
            let Some(&op_i) = violating.get(slot as usize) else {
                continue;
            };
            if rep
                .get("isolated")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                continue; // explicit isolation accepted
            }
            let mut new_links: Vec<String> = rep
                .get("links")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|l| l.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let mut stats = ResolveStats::default();
            refs.resolve_links(&mut new_links, &mut stats);
            if stats.dropped_links > 0 {
                warn!(
                    dropped = stats.dropped_links,
                    "link contract repair: dropped hallucinated tokens"
                );
            }
            if new_links.is_empty() {
                continue;
            }
            if let PageOp::Create { links, .. } = &mut ops[op_i] {
                links.extend(new_links);
                links.dedup();
                repaired += 1;
            }
        }
        if repaired > 0 {
            info!(repaired, "link contract: repaired linkless creates");
        }
        ops
    }
```

- [ ] **Step 4: 两处接线**

`ingest_batch` 主路径（现 `ingestor.rs:367-369`，`dedup_redirect_creates` 调用后、`if self.gate.is_some()` 前）：

```rust
        plan.ops = self
            .dedup_redirect_creates(agent_id, plan.ops, &related)
            .await;

        // Link-contract harmony gate: repair linkless Creates (or accept an
        // explicit isolation) before governance gating. Runs after dedup so a
        // Create already redirected into an Append is not re-examined.
        plan.ops = self.enforce_link_contract(plan.ops, &related).await;
```

hash-conflict 重放路径（现 `ingestor.rs:400-402`，`plan2.ops = ... dedup_redirect_creates ...` 之后）同样加：

```rust
                plan2.ops = self.enforce_link_contract(plan2.ops, &related).await;
```

- [ ] **Step 5: 跑测试**

Run: `cargo test -p alephcore --lib link_contract_tests && cargo test -p alephcore --lib ingest`
Expected: 新测试 6 个全 PASS；既有 ingest 测试全 PASS（既有 `ingest_batch` 测试中 mock embedder + 空库 → `gather_related` 返回空 related → 门为 no-op，不受影响）

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/ingest/ingestor.rs src/memory/notes/ingest/ref_table.rs
git commit -m "memory: add enforce_link_contract harmony gate to compound ingest"
```

---

### Task 4: `note_manage` create 返回 related_notes 候选

**Files:**
- Modify: `src/builtin_tools/note_manage.rs`（`NoteManageResult` 新字段 + `handle_create` 检索 + DESCRIPTION 强化 + 单测）

- [ ] **Step 1: 写失败的单测**

在 `note_manage.rs` 现有 `mod tests` 中追加（fixture 仿 `NoteManageTool::new(memory_dir, store)`）：

```rust
    use crate::memory::store::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;

    fn mk_tool() -> (tempfile::TempDir, NoteManageTool) {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let tool = NoteManageTool::new(dir.path().join("note"), backend);
        (dir, tool)
    }

    fn create_args(filename: &str, content: &str) -> NoteManageArgs {
        NoteManageArgs {
            action: NoteManageAction::Create,
            category: Some("learning".into()),
            filename: Some(filename.into()),
            title: Some(filename.into()),
            content: Some(content.into()),
            // 其余字段按 NoteManageArgs 定义补 None / 默认值
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn create_surfaces_related_notes() {
        let (_d, tool) = mk_tool();
        // 第一篇：含独特词 tokio-runtime
        let r1 = tool
            .call(create_args("tokio-basics", "- tokio-runtime event loop basics"))
            .await
            .unwrap();
        assert!(r1.success);
        // 第二篇内容与第一篇高度相关 → related_notes 应包含第一篇
        let r2 = tool
            .call(create_args(
                "tokio-advanced",
                "- advanced tokio-runtime scheduling patterns",
            ))
            .await
            .unwrap();
        assert!(r2.success);
        let related = r2.related_notes.expect("related notes should surface");
        assert!(
            related.iter().any(|n| n.path == "learning/tokio-basics"),
            "expected learning/tokio-basics in {related:?}"
        );
        // 自身不出现在候选里
        assert!(related.iter().all(|n| n.path != "learning/tokio-advanced"));
        // message 携带 nudge
        assert!(r2.message.contains("consider linking"));
    }

    #[tokio::test]
    async fn create_with_no_related_notes_omits_field() {
        let (_d, tool) = mk_tool();
        let r = tool
            .call(create_args("zzz-unique", "- completely unrelated xyzzy fact"))
            .await
            .unwrap();
        assert!(r.success);
        assert!(r.related_notes.is_none());
    }
```

注意：若 `NoteManageArgs` 未派生 `Default`，为 args 补一个测试侧构造（逐字段写 None）即可，不要给生产 struct 加 derive（除非它已有）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib note_manage`
Expected: 编译 FAIL（`related_notes` 字段不存在）

- [ ] **Step 3: 实现**

3a. `NoteManageResult`（`note_manage.rs:132`）加字段：

```rust
    /// Related existing notes surfaced after a create, so the model can
    /// weave the new note into the wiki (via `links`) instead of leaving an
    /// orphan island.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_notes: Option<Vec<NoteListEntry>>,
```

3b. 全部既有 `NoteManageResult { ... }` 构造点补 `related_notes: None,`（编译器 E0063 会逐一列出：update/append/query/list/delete 各 handler）。

3c. `handle_create` 末尾（`index_file` 成功后、构造返回值前）替换原 `Ok(NoteManageResult {...})`：

```rust
        let note_path = format!("{category}/{safe_filename}");
        info!(path = %note_path, "Note created");

        // Surface related existing notes (best-effort, FTS-only — this tool
        // has no embedder) so the model can weave the new note into the wiki
        // instead of leaving an orphan island. Search failure must never
        // fail the create.
        let query_text: String = format!(
            "{} {}",
            args.title.as_deref().unwrap_or(&safe_filename),
            args.content.as_deref().unwrap_or("")
        )
        .chars()
        .take(200)
        .collect();
        let related_notes = match self
            .indexer
            .store()
            .search_notes_fts(&query_text, agent_id, 6)
            .await
        {
            Ok(hits) => {
                let rel: Vec<NoteListEntry> = hits
                    .into_iter()
                    .filter(|e| e.path != note_path)
                    .take(5)
                    .map(|e| NoteListEntry {
                        path: e.path,
                        category: e.category,
                        filename: e.filename,
                        tags: e.tags,
                    })
                    .collect();
                (!rel.is_empty()).then_some(rel)
            }
            Err(e) => {
                warn!(error = %e, "note_manage create: related-note search failed");
                None
            }
        };
        let message = match &related_notes {
            Some(rel) => format!(
                "Created note '{safe_filename}' in '{category}'. Found {} related note(s) — consider linking them (append with links=[...]) so this note is not an orphan: {}",
                rel.len(),
                rel.iter()
                    .map(|r| r.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None => format!("Created note '{safe_filename}' in '{category}'"),
        };

        Ok(NoteManageResult {
            success: true,
            message,
            note_path: Some(note_path),
            content: None,
            notes: None,
            related_notes,
        })
```

3d. `DESCRIPTION` 常量强化（替换原文案）：

```rust
    const DESCRIPTION: &'static str =
        "Create, update, append, query, list, or delete personal knowledge notes. \
         Notes are markdown files organized by category (preference, plan, learning, \
         project, personal, tool, lesson, skill, reference, transcript, other). \
         Use this tool to store and retrieve long-term knowledge and preferences. \
         IMPORTANT: notes form a wiki — when creating a note, ALWAYS connect it to \
         related notes via the `links` parameter; linkless notes become orphan \
         islands and are archived early. The create result returns `related_notes` \
         candidates — link the relevant ones with a follow-up append.";
```

- [ ] **Step 4: 跑测试**

Run: `cargo test -p alephcore --lib note_manage`
Expected: 全 PASS。若 `create_surfaces_related_notes` 因 FTS 未命中失败，先检查 `search_notes_fts` 对连字符 query 的转义（必要时把测试用词改为无连字符的 `tokioruntime`），不要改生产逻辑。

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/note_manage.rs
git commit -m "tools: surface related notes after note_manage create"
```

---

### Task 5: `NoteWeaveStage` —— 孤岛编织 dream stage

**Files:**
- Create: `src/memory/dreaming/stages/note_weave.rs`
- Modify: `src/memory/dreaming/report.rs`（`notes_woven` 字段）

- [ ] **Step 1: DreamReport 加字段**

`report.rs` 的 `DreamReport`（`notes_archived` / `notes_protected` 之后）：

```rust
    /// Orphan notes successfully woven into the link graph by `NoteWeave`.
    #[serde(default)]
    pub notes_woven: u32,
```

（struct 派生 `Default`，字面量构造点若有编译错误按 E0063 补 `notes_woven: 0`。）

- [ ] **Step 2: 写新 stage 文件（含单测）**

创建 `src/memory/dreaming/stages/note_weave.rs`：

```rust
//! NoteWeave stage — weave orphan notes into the wiki link graph.
//!
//! An orphan (zero outgoing AND zero incoming links) is invisible to graph
//! expansion in `gather_related`, earns no `link_weight` in `NoteDecay`
//! scoring, and is therefore archived early — a vicious cycle this stage
//! breaks. Detection is store-query-only (cheap); per orphan, one LLM call
//! picks 0-3 targets from hybrid-search candidates (R7 — the model decides
//! who to link, the harness only validates `[C<n>]` tokens against the
//! candidate set, mirroring the ingest `RefTable` anti-hallucination line).
//! Links are written through `NoteIndexer::append_to_note` in both
//! directions, the same primitive `CompoundApplyTx::add_link` uses.

use async_trait::async_trait;
use tracing::{info, warn};

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::store::NoteStore;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::utils::json_extract::extract_json_robust;

use super::DreamStage;

/// Max orphan notes processed (one LLM call each) per dream cycle.
const MAX_WEAVE_PER_CYCLE: usize = 10;
/// Max link candidates shown to the LLM per orphan.
const MAX_CANDIDATES: usize = 8;
/// Max links accepted per orphan.
const MAX_LINKS_PER_NOTE: usize = 3;

const PROMPT_WEAVE: &str = r#"You maintain an Aleph personal-memory wiki.
The note below is an ORPHAN — no other note links to it and it links to
none. From the candidate pages, pick 0-3 that genuinely relate to it.

Rules:
- Only use `[C<n>]` tokens that appear in the "Candidates" section.
- Pick a candidate only when a reader of one note would benefit from the
  other; do NOT force a link.
- When nothing truly relates, return an empty list.
- Output valid JSON only, no prose: {"links": ["[C1]", "[C4]"]}
"#;

/// NoteWeave stage. `max_per_cycle` caps LLM spend per dream cycle.
pub struct NoteWeaveStage {
    pub max_per_cycle: usize,
}

impl Default for NoteWeaveStage {
    fn default() -> Self {
        Self {
            max_per_cycle: MAX_WEAVE_PER_CYCLE,
        }
    }
}

#[async_trait]
impl DreamStage for NoteWeaveStage {
    fn name(&self) -> &'static str {
        "note_weave"
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // --- Phase 1: orphan detection (store queries only, no LLM) ---
        let mut orphans: Vec<String> = Vec::new();
        for note in &ctx.notes {
            if orphans.len() >= self.max_per_cycle {
                break;
            }
            let Some((_, filename)) = note.path.split_once('/') else {
                continue;
            };
            let outgoing = ctx
                .indexer
                .store()
                .get_outgoing_links(&note.path, &ctx.agent_id)
                .await
                .unwrap_or_default();
            if !outgoing.is_empty() {
                continue;
            }
            // notes_links stores raw wikilink targets by filename — query by
            // filename, mirroring NoteDecay's incoming-link count.
            let incoming = ctx
                .indexer
                .store()
                .get_incoming_links(filename, &ctx.agent_id)
                .await
                .unwrap_or_default();
            if !incoming.is_empty() {
                continue;
            }
            orphans.push(note.path.clone());
        }
        if orphans.is_empty() {
            info!("NoteWeave: no orphan notes");
            return Ok(ctx);
        }
        info!(count = orphans.len(), "NoteWeave: found orphan notes");

        // --- Phase 2: weave each orphan (one LLM call each; per-note
        // failure skips that note only). ---
        let mut woven = 0u32;
        for path in orphans {
            match weave_one(&mut ctx, &path).await {
                Ok(n) => woven += n,
                Err(e) => warn!(path = %path, error = %e, "NoteWeave: skipped orphan"),
            }
        }
        ctx.report.notes_woven = woven;
        info!(woven, "NoteWeave completed");
        Ok(ctx)
    }
}

/// Weave a single orphan: gather candidates via hybrid search over the
/// orphan's own content, ask the LLM to pick targets, write bidirectional
/// links. Returns the number of links written.
async fn weave_one(ctx: &mut DreamContext, path: &str) -> Result<u32, AlephError> {
    let Some(content) = ctx.load_content(path).await else {
        return Ok(0);
    };
    let embedding = ctx.embedder.embed(&content).await?;
    let dim = embedding.len() as u32;
    let hits = ctx
        .indexer
        .store()
        .hybrid_search_notes(&embedding, &content, &ctx.agent_id, dim, MAX_CANDIDATES + 1)
        .await?;
    let candidates: Vec<_> = hits
        .into_iter()
        .filter(|h| h.path != path)
        .take(MAX_CANDIDATES)
        .collect();
    if candidates.is_empty() {
        return Ok(0); // genuinely alone — nothing to weave
    }

    let mut user = format!("## Orphan note: {path}\n\n");
    for line in content.lines().take(40) {
        user.push_str(line);
        user.push('\n');
    }
    user.push_str("\n## Candidates\n\n");
    for (i, c) in candidates.iter().enumerate() {
        let preview: String = c.content.chars().take(200).collect();
        user.push_str(&format!("[C{i}] {} — {}\n", c.path, preview.replace('\n', " ")));
    }

    let msgs = [UnifiedMessage::user(&user)];
    let resp = ctx
        .provider
        .process(RequestPayload::new(&msgs).with_system(Some(PROMPT_WEAVE)))
        .await
        .map_err(|e| AlephError::other(format!("weave LLM: {e}")))?;
    let Some(json) = extract_json_robust(&resp.text_content()) else {
        return Ok(0);
    };
    let targets = parse_weave_targets(&json, candidates.len());

    let mut written = 0u32;
    for idx in targets.into_iter().take(MAX_LINKS_PER_NOTE) {
        let target = candidates[idx].path.clone();
        // Bidirectional, mirroring CompoundApplyTx::add_link.
        ctx.indexer
            .append_to_note(&ctx.agent_id, path, &Vec::<String>::new(), &[target.clone()])
            .await?;
        ctx.indexer
            .append_to_note(&ctx.agent_id, &target, &Vec::<String>::new(), &[path.to_string()])
            .await?;
        // Evict cached bodies so downstream stages re-read updated markdown.
        ctx.note_contents.remove(path);
        ctx.note_contents.remove(target.as_str());
        written += 1;
    }
    Ok(written)
}

/// Parse `{"links": ["[C1]", ...]}` into in-range candidate indices.
/// Out-of-range, malformed, or duplicate tokens are dropped
/// (anti-hallucination, mirroring the ingest `RefTable` line).
fn parse_weave_targets(json: &serde_json::Value, n_candidates: usize) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    let Some(arr) = json.get("links").and_then(|v| v.as_array()) else {
        return out;
    };
    for v in arr {
        let Some(s) = v.as_str() else { continue };
        let Some(inner) = s.trim().strip_prefix("[C").and_then(|r| r.strip_suffix(']')) else {
            continue;
        };
        if inner.is_empty() || !inner.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(idx) = inner.parse::<usize>() else { continue };
        if idx < n_candidates && !out.contains(&idx) {
            out.push(idx);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_name_is_note_weave() {
        assert_eq!(NoteWeaveStage::default().name(), "note_weave");
    }

    #[test]
    fn default_cap_is_ten() {
        assert_eq!(NoteWeaveStage::default().max_per_cycle, 10);
    }

    #[test]
    fn parse_targets_accepts_in_range_tokens() {
        let j: serde_json::Value = serde_json::json!({"links": ["[C0]", "[C2]"]});
        assert_eq!(parse_weave_targets(&j, 3), vec![0, 2]);
    }

    #[test]
    fn parse_targets_drops_out_of_range_and_malformed() {
        let j: serde_json::Value =
            serde_json::json!({"links": ["[C9]", "[P1]", "C1", "[C]", 42, "[C1]"]});
        assert_eq!(parse_weave_targets(&j, 3), vec![1]);
    }

    #[test]
    fn parse_targets_dedups() {
        let j: serde_json::Value = serde_json::json!({"links": ["[C1]", "[C1]"]});
        assert_eq!(parse_weave_targets(&j, 3), vec![1]);
    }

    #[test]
    fn parse_targets_empty_or_missing() {
        assert!(parse_weave_targets(&serde_json::json!({"links": []}), 3).is_empty());
        assert!(parse_weave_targets(&serde_json::json!({}), 3).is_empty());
    }

    // -----------------------------------------------------------------
    // Stage-level test: orphan detection finds linked vs unlinked notes.
    // Fixture mirrors note_lint.rs::build_test_dream_ctx.
    // -----------------------------------------------------------------

    use crate::memory::dreaming::{DreamContext, NoteEntry};
    use crate::memory::embedding_provider::EmbeddingProvider;
    use crate::memory::notes::{KnowledgeNote, NoteIndexer};
    use crate::memory::store::SqliteMemoryBackend;
    use crate::providers::mock::MockProvider;
    use crate::sync_primitives::Arc;

    struct StubEmbedder;

    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
            Ok(Vec::new())
        }
        async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(Vec::new())
        }
        fn dimensions(&self) -> usize {
            0
        }
        fn model_name(&self) -> &str {
            "stub"
        }
        fn provider_id(&self) -> &str {
            "stub"
        }
    }

    async fn build_ctx(llm_response: &str) -> (DreamContext, Arc<SqliteMemoryBackend>) {
        let temp = std::env::temp_dir().join(format!("aleph_weave_{}", uuid::Uuid::new_v4()));
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let indexer = NoteIndexer::new(temp.clone(), store.clone());
        let provider: std::sync::Arc<dyn crate::providers::AiProvider> =
            std::sync::Arc::new(MockProvider::new(llm_response));
        let embedder: std::sync::Arc<dyn EmbeddingProvider> = std::sync::Arc::new(StubEmbedder);
        let ctx = DreamContext {
            notes: Vec::new(),
            note_contents: std::collections::HashMap::new(),
            agent_id: "default".into(),
            database: store.clone(),
            indexer,
            provider,
            embedder,
            report: crate::memory::dreaming::DreamReport::default(),
            pipeline_type: "consolidate".into(),
            activity_checker: std::sync::Arc::new(|| false),
            strategy: crate::memory::dreaming::DreamStrategy::Consolidate,
            orientation: None,
        };
        (ctx, store)
    }

    fn entry(path: &str) -> NoteEntry {
        let (category, _) = path.split_once('/').unwrap();
        NoteEntry {
            path: path.into(),
            category: category.into(),
            tags: vec![],
            created_at: 0,
            updated_at: 0,
            content_hash: "h".into(),
        }
    }

    #[tokio::test]
    async fn linked_note_is_not_treated_as_orphan() {
        let (mut ctx, store) = build_ctx("{\"links\": []}").await;
        // a links to b → neither is an orphan.
        store
            .index_note(
                &KnowledgeNote {
                    title: "a".into(),
                    category: "learning".into(),
                    facts: vec!["see [[b]]".into()],
                    links: vec!["learning/b".into()],
                    content_hash: "h1".into(),
                    ..Default::default()
                },
                "default",
                "learning",
            )
            .await
            .unwrap();
        store
            .index_note(
                &KnowledgeNote {
                    title: "b".into(),
                    category: "learning".into(),
                    facts: vec!["fact".into()],
                    content_hash: "h2".into(),
                    ..Default::default()
                },
                "default",
                "learning",
            )
            .await
            .unwrap();
        ctx.notes = vec![entry("learning/a"), entry("learning/b")];

        let out = NoteWeaveStage::default().execute(ctx).await.unwrap();
        // No orphans → no weave writes recorded.
        assert_eq!(out.report.notes_woven, 0);
    }

    #[tokio::test]
    async fn orphan_with_no_disk_file_is_skipped_gracefully() {
        // Orphan detected in the index but its markdown file does not exist →
        // load_content returns None → weave_one returns 0, stage completes.
        let (mut ctx, store) = build_ctx("{\"links\": [\"[C0]\"]}").await;
        store
            .index_note(
                &KnowledgeNote {
                    title: "lonely".into(),
                    category: "learning".into(),
                    facts: vec!["isolated fact".into()],
                    content_hash: "h1".into(),
                    ..Default::default()
                },
                "default",
                "learning",
            )
            .await
            .unwrap();
        ctx.notes = vec![entry("learning/lonely")];

        let out = NoteWeaveStage::default().execute(ctx).await.unwrap();
        assert_eq!(out.report.notes_woven, 0);
    }
}
```

注意：`MockProvider::new(...)` 的参数类型以 `src/providers/mock.rs` 实际签名为准（note_lint.rs:498 用 `MockProvider::new("")`，同款即可）。`database: store.clone()` 的类型同 note_lint fixture。

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p alephcore --lib note_weave`
Expected: 编译 FAIL（模块未注册——Task 6 注册后才能编译；可先做 Task 6 Step 1 再回来跑红）

- [ ] **Step 4: Commit（与 Task 6 合并提交，见下）**

---

### Task 6: 注册 stage 进 Consolidate 管线

**Files:**
- Modify: `src/memory/dreaming/stages/mod.rs`（模块声明 + re-export）
- Modify: `src/memory/dreaming/mod.rs:161-186`（Consolidate 管线插入）

- [ ] **Step 1: stages/mod.rs 注册**

```rust
pub mod note_weave;
// ...
pub use note_weave::NoteWeaveStage;
```

（按字母序插在 `note_synthesis` 之后、`skill_distill` 之前的现有声明/导出区。）

- [ ] **Step 2: 管线插入**

`src/memory/dreaming/mod.rs` 的 `DreamStrategy::Consolidate` 列表中，`Box::new(note_decay())` 之前插入：

```rust
                // Weave orphan notes into the link graph BEFORE decay scores
                // them: a freshly woven link immediately counts toward
                // link_weight / the >=3-incoming-links protection, breaking
                // the orphan→no-link-weight→archived vicious cycle.
                Box::new(stages::NoteWeaveStage::default()),
```

不加入 `GLOBAL_ONLY_STAGES`（编织是 per-agent/per-namespace 的 note 维护，project 命名空间同样适用）。

- [ ] **Step 3: 跑测试**

Run: `cargo test -p alephcore --lib note_weave && cargo test -p alephcore --lib dreaming`
Expected: note_weave 单测全 PASS；dreaming 既有测试全 PASS

- [ ] **Step 4: Commit**

```bash
git add src/memory/dreaming/stages/note_weave.rs src/memory/dreaming/stages/mod.rs src/memory/dreaming/mod.rs src/memory/dreaming/report.rs
git commit -m "memory: add NoteWeave dream stage to relink orphan notes"
```

---

### Task 7: 全量验证

- [ ] **Step 1: 编译 + lint**

Run: `cargo check -p alephcore && cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -20`
Expected: 零错误零警告（若 clippy 报既有代码的预存警告，只修本轮新增代码触发的）

- [ ] **Step 2: 核心测试全量**

Run: `cargo test -p alephcore --lib`
Expected: 全 PASS

- [ ] **Step 3: rustfmt（注意 ~/.claude hook 可能已全仓格式化——若 diff 混入无关文件的纯格式化改动，`git checkout -- <无关文件>` 还原）**

Run: `cargo fmt -p alephcore && git status --short`
Expected: 仅本轮触碰的文件有改动

- [ ] **Step 4: 最终提交（若 fmt 有残留改动）**

```bash
git add -u && git commit -m "memory: fmt for link weaving round" || true
```

---

## Self-Review 记录

- **Spec 覆盖**：§2.1→Task 1；§2.2→Task 2+3（含 hash-conflict 重放路径接线）；§3→Task 4；§4→Task 5+6（注册位置 NoteDecay 前）；§5 测试→各 task TDD 步骤；§6 不做项未引入。
- **类型一致性**：`enforce_link_contract(Vec<PageOp>, &[RelatedPage]) -> Vec<PageOp>` 两个调用点签名一致；`parse_weave_targets(&Value, usize) -> Vec<usize>` 测试与实现一致；`related_notes: Option<Vec<NoteListEntry>>` 测试断言字段名一致。
- **已知风险**：① Task 4 FTS 对连字符 query 的行为未实测——测试步骤已给降级指引；② Task 5 stage 测试不走真实 hybrid search 路径（StubEmbedder 0 维），weave_one 的 LLM/写链分支由 parse 单测 + graceful-skip 测试覆盖，端到端行为留给 dream 周期实际运行验证；③ `NoteManageArgs` 是否派生 `Default` 需现场确认，测试构造按需调整。
