# Chat/Config 权限分层 Phase 3b-2a Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Operator 在 Panel 通知中心看到挂起的操作授权请求（config 工具名 + 请求方 agent + 过期），一键 批准一次/本会话批准/拒绝。

**Architecture:** 纯 Panel UI（零后端改动）。本地 Panel 以 operator(`*`) 连接已能收 `approval.*` 事件；`exec.approvals.pending`（开放读）是卡片内容 SSOT，`approval.*` 事件仅触发重拉；决策经 operator-gated `exec.approval.resolve` 下发（decision kebab-case）。

**Tech Stack:** Leptos/WASM Panel（验证 = `just wasm`，不 cargo-check）、leptos-i18n（en/zh 并行）。零 Rust 后端改动。

**Spec:** `docs/superpowers/specs/2026-06-07-chat-config-permission-tier-phase3b2a-design.md`

**Git 约束（全程）:** 共享单分支 main + 并发提交者——只追加式提交、**显式文件路径**暂存（禁 `git add -A/-u/.`）、禁 reset/amend/rebase/push；提交信息英文、无 attribution footer；不 push；提交前 `git status` 确认不卷入他人 WIP（工作区有 dist 产物未暂存，勿 staged）。

---

## File Structure

- `interfaces/webchat/src/api/exec_approval.rs`（新）— `ExecApprovalApi::{list_pending, resolve}`。
- `interfaces/webchat/src/api.rs` — 加 `pub mod exec_approval;` + `pub use exec_approval::*;`。
- `interfaces/webchat/src/state/notifications.rs` — 新增 `PendingApprovalView`。
- `interfaces/webchat/src/context.rs` — `pending_approvals` 信号 + `approval_subscription_id` + `setup_approval_subscriptions`。
- `interfaces/webchat/src/app.rs` — 调用 `setup_approval_subscriptions`（:104 同址）。
- `interfaces/webchat/locales/{en,zh}.json` — 审批卡 i18n 键。
- `interfaces/webchat/src/components/notification_center.rs` — 审批卡区 + badge 计数。

任务顺序：Task 1（API+state 基础）→ Task 2（context 订阅，依赖 Task 1）→ Task 3（i18n 键，组件引用前）→ Task 4（通知卡，依赖 1/2/3）。Task 1/2 落地后会有短暂 dead_code 警告（未被消费），`just wasm` 仍成功；Task 4 接通后消失。

---

### Task 1: Panel API + state — ExecApprovalApi + PendingApprovalView

**Files:**
- Create: `interfaces/webchat/src/api/exec_approval.rs`
- Modify: `interfaces/webchat/src/api.rs`（加 mod + re-export）
- Modify: `interfaces/webchat/src/state/notifications.rs`（加 `PendingApprovalView`）

- [ ] **Step 1: `state/notifications.rs` 加 `PendingApprovalView`**

在 `IncomingPairing` 定义之后追加：

```rust
/// A pending operator-approval request rendered by the NotificationCenter with
/// inline allow-once / allow-session / deny buttons. Sourced from the
/// `exec.approvals.pending` RPC (the `approval.**` events are sparse — they only
/// trigger a refetch). Display-only.
#[derive(Debug, Clone)]
pub struct PendingApprovalView {
    /// Approval request id (passed to `exec.approval.resolve`).
    pub id: String,
    /// The config tool name being requested (ExecApprovalRecord.command).
    pub command: String,
    /// The requesting agent id.
    pub agent_id: String,
    /// Milliseconds until the approval times out.
    pub remaining_ms: u64,
}
```

- [ ] **Step 2: 创建 `api/exec_approval.rs`**

```rust
use crate::context::DashboardState;
use crate::state::notifications::PendingApprovalView;
use serde::Deserialize;

// ============================================================================
// Exec Approval API (operator approval card)
// ============================================================================

pub struct ExecApprovalApi;

#[derive(Deserialize)]
struct PendingListResp {
    pending: Vec<PendingItem>,
}

#[derive(Deserialize)]
struct PendingItem {
    record: PendingRecord,
    remaining_ms: u64,
}

#[derive(Deserialize)]
struct PendingRecord {
    id: String,
    command: String,
    agent_id: String,
}

impl ExecApprovalApi {
    /// List pending operator approvals (the source of truth for the cards).
    pub async fn list_pending(
        state: &DashboardState,
    ) -> Result<Vec<PendingApprovalView>, String> {
        let result = state
            .rpc_call("exec.approvals.pending", serde_json::Value::Null)
            .await?;
        let resp: PendingListResp = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse pending approvals: {}", e))?;
        Ok(resp
            .pending
            .into_iter()
            .map(|p| PendingApprovalView {
                id: p.record.id,
                command: p.record.command,
                agent_id: p.record.agent_id,
                remaining_ms: p.remaining_ms,
            })
            .collect())
    }

    /// Resolve a pending approval. `decision` is the kebab-case wire value:
    /// "allow-once" | "allow-session" | "deny".
    pub async fn resolve(
        state: &DashboardState,
        id: String,
        decision: &str,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "id": id,
            "decision": decision,
            "resolved_by": "Operator (Panel)",
        });
        state.rpc_call("exec.approval.resolve", params).await?;
        Ok(())
    }
}
```

- [ ] **Step 3: `api.rs` 注册模块**

在 `pub mod security;`（约 :33）附近加：
```rust
pub mod exec_approval;
```
在 `pub use security::*;`（约 :59）附近加：
```rust
pub use exec_approval::*;
```

- [ ] **Step 4: WASM 构建验证**

Run: `just wasm 2>&1 | tail -25`
Expected: 构建成功（可能有 `ExecApprovalApi`/`PendingApprovalView` 未使用的 dead_code 警告——本任务尚未接通，正常，Task 2/4 消费后消失）。

- [ ] **Step 5: 提交（显式路径）**

```bash
git add interfaces/webchat/src/api/exec_approval.rs interfaces/webchat/src/api.rs interfaces/webchat/src/state/notifications.rs
git commit -m "panel: exec approval api + pending-approval view model"
```

---

### Task 2: context.rs — pending_approvals 信号 + 订阅

**Files:**
- Modify: `interfaces/webchat/src/context.rs`（字段 ~:115/:118、构造 ~:170、imports、新方法）
- Modify: `interfaces/webchat/src/app.rs`（:104 同址调用）

- [ ] **Step 1: 确保 imports**

在 `context.rs` 顶部 import 区加（若缺）：
```rust
use crate::api::ExecApprovalApi;
use crate::state::notifications::PendingApprovalView;
```
（`spawn_local` 已在 context.rs:7 导入；`IncomingPairing`/`GatewayEvent` 已在用。若 `PendingApprovalView` 所在 `state::notifications` 已有部分 import，合并即可。）

- [ ] **Step 2: `DashboardState` 加字段**

在 `incoming_pairings`（:115）/ `pairing_subscription_id`（:118）之后追加：
```rust
    /// Pending operator-approval requests rendered by the NotificationCenter
    /// with inline allow-once / allow-session / deny buttons. Sourced from the
    /// `exec.approvals.pending` RPC; `approval.**` events trigger a refetch
    /// (see `setup_approval_subscriptions`).
    pub pending_approvals: RwSignal<Vec<PendingApprovalView>>,

    /// Approval subscription ID for cleanup.
    approval_subscription_id: StoredValue<Option<usize>>,
```

- [ ] **Step 3: 构造默认值**

在 `incoming_pairings: RwSignal::new(Vec::new()),` / `pairing_subscription_id: StoredValue::new(None),`（:170-171）之后追加：
```rust
            pending_approvals: RwSignal::new(Vec::new()),
            approval_subscription_id: StoredValue::new(None),
```

- [ ] **Step 4: 新增 `setup_approval_subscriptions`**

在 `setup_pairing_subscriptions`（结束于 :834）之后追加：
```rust
    /// Subscribe to `approval.**` events so the NotificationCenter can render
    /// inline operator approval cards. The ApprovalRequested event is sparse
    /// (ids only), so `exec.approvals.pending` is the source of truth: any
    /// approval event simply triggers a refetch.
    pub async fn setup_approval_subscriptions(&self) -> Result<(), String> {
        self.subscribe_topic("approval.**").await?;
        web_sys::console::log_1(&"Subscribed to approval.** events".into());

        // Seed with whatever is already pending at connect time.
        if let Ok(list) = ExecApprovalApi::list_pending(self).await {
            self.pending_approvals.set(list);
        }

        let state = *self;
        let subscription_id = self.subscribe_events(move |event: GatewayEvent| {
            match event.topic.as_str() {
                "approval.requested" | "approval.resolved" | "approval.expired" => {
                    spawn_local(async move {
                        if let Ok(list) = ExecApprovalApi::list_pending(&state).await {
                            state.pending_approvals.set(list);
                        }
                    });
                }
                _ => {}
            }
        });

        self.approval_subscription_id
            .set_value(Some(subscription_id));
        Ok(())
    }
```

- [ ] **Step 5: `app.rs` 调用**

在 `setup_pairing_subscriptions` 调用块（app.rs:104-108）之后追加：
```rust
                    if let Err(e) = state.setup_approval_subscriptions().await {
                        web_sys::console::error_1(
                            &format!("Failed to setup approval subscriptions: {}", e).into(),
                        );
                    }
```

- [ ] **Step 6: WASM 构建验证**

Run: `just wasm 2>&1 | tail -25`
Expected: 构建成功（`pending_approvals` 信号尚未被 UI 读取 → 仍可能有 dead_code/unused 警告，Task 4 接通后消失）。

- [ ] **Step 7: 提交（显式路径）**

```bash
git add interfaces/webchat/src/context.rs interfaces/webchat/src/app.rs
git commit -m "panel: subscribe approval.** + pending-approvals signal"
```

---

### Task 3: i18n 键（en.json + zh.json 并行）

**Files:**
- Modify: `interfaces/webchat/locales/en.json`、`interfaces/webchat/locales/zh.json`

> leptos-i18n 编译期校验 `t!` 键且要求 en/zh 键集一致——本任务先于 Task 4。

- [ ] **Step 1: en.json `notifications` 段加键**

在 `notifications` 对象内（如 `"empty"` 之后）加：
```json
    "approval_header": "Operator authorization",
    "approval_requested_by": "Requested by",
    "approval_expires": "Expires in",
    "approval_allow_once": "Approve once",
    "approval_allow_session": "Approve for session",
    "approval_deny": "Deny",
```

- [ ] **Step 2: zh.json `notifications` 段加同名键**

```json
    "approval_header": "操作授权",
    "approval_requested_by": "请求方",
    "approval_expires": "剩余",
    "approval_allow_once": "批准一次",
    "approval_allow_session": "本会话批准",
    "approval_deny": "拒绝",
```

- [ ] **Step 3: JSON 合法 + 键集一致检查**

Run:
```bash
cd interfaces/webchat/locales && python3 -c "
import json
def keys(d,p=''):
    s=set()
    for k,v in d.items():
        s.add(p+k)
        if isinstance(v,dict): s|=keys(v,p+k+'.')
    return s
e=keys(json.load(open('en.json'))); z=keys(json.load(open('zh.json')))
print('en-only:', sorted(e-z)); print('zh-only:', sorted(z-e)); print('match:', e==z)
"
```
Expected: `en-only: []`，`zh-only: []`，`match: True`

- [ ] **Step 4: 提交（显式路径）**

```bash
git add interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "panel: i18n keys for operator approval card"
```

---

### Task 4: notification_center.rs — 审批卡区 + badge

**Files:**
- Modify: `interfaces/webchat/src/components/notification_center.rs`（imports、信号绑定 ~:31、badge ~:44、配对区之后插审批区）

- [ ] **Step 1: imports**

顶部加（若缺）：
```rust
use crate::api::ExecApprovalApi;
use crate::state::notifications::PendingApprovalView;
```

- [ ] **Step 2: 绑定信号 + badge 计数**

在 `let incoming_pairings = dashboard.incoming_pairings;`（~:31）之后加：
```rust
    let pending_approvals = dashboard.pending_approvals;
```
把 `badge_count` Memo（~:44）的求和改为：
```rust
        unread_count(&a, &d) + incoming_pairings.get().len() + pending_approvals.get().len()
```

- [ ] **Step 3: 插入审批卡区**

在配对列表 `{move || { let pairings = incoming_pairings.get(); ... }}` 整块之后、系统告警 `{move || { let items = list.get(); ... }}` 整块之前，插入：

```rust
                    {move || {
                        let approvals = pending_approvals.get();
                        if approvals.is_empty() {
                            view! { <div></div> }.into_any()
                        } else {
                            view! {
                                <ul class="divide-y divide-border">
                                    {approvals.into_iter().map(|a: PendingApprovalView| {
                                        let i18n = use_i18n();
                                        let id_once = a.id.clone();
                                        let id_session = a.id.clone();
                                        let id_deny = a.id.clone();
                                        let command = a.command.clone();
                                        let agent_id = a.agent_id.clone();
                                        let secs = (a.remaining_ms / 1000).to_string();
                                        view! {
                                            <li class="px-4 py-3">
                                                <div class="text-sm font-medium text-text-primary">
                                                    {t!(i18n, notifications.approval_header)}
                                                </div>
                                                <div class="font-mono text-sm my-1 text-indigo-300">
                                                    {command}
                                                </div>
                                                <div class="text-xs text-text-secondary">
                                                    {t!(i18n, notifications.approval_requested_by)} ": " {agent_id}
                                                </div>
                                                <div class="text-xs text-text-tertiary mt-0.5">
                                                    {t!(i18n, notifications.approval_expires)} " " {secs} "s"
                                                </div>
                                                <div class="flex gap-2 mt-2">
                                                    <button
                                                        type="button"
                                                        class="flex-1 py-1.5 rounded bg-indigo-600 hover:bg-indigo-500 text-white text-xs font-semibold transition-colors"
                                                        on:click=move |_| {
                                                            let id = id_once.clone();
                                                            spawn_local(async move {
                                                                let _ = ExecApprovalApi::resolve(&dashboard, id.clone(), "allow-once").await;
                                                                dashboard.pending_approvals.update(|l| l.retain(|x| x.id != id));
                                                            });
                                                        }
                                                    >
                                                        {t!(i18n, notifications.approval_allow_once)}
                                                    </button>
                                                    <button
                                                        type="button"
                                                        class="flex-1 py-1.5 rounded bg-surface-raised hover:bg-surface-sunken text-text-primary text-xs border border-border transition-colors"
                                                        on:click=move |_| {
                                                            let id = id_session.clone();
                                                            spawn_local(async move {
                                                                let _ = ExecApprovalApi::resolve(&dashboard, id.clone(), "allow-session").await;
                                                                dashboard.pending_approvals.update(|l| l.retain(|x| x.id != id));
                                                            });
                                                        }
                                                    >
                                                        {t!(i18n, notifications.approval_allow_session)}
                                                    </button>
                                                    <button
                                                        type="button"
                                                        class="flex-1 py-1.5 rounded bg-surface-sunken hover:bg-surface-raised text-text-secondary text-xs transition-colors"
                                                        on:click=move |_| {
                                                            let id = id_deny.clone();
                                                            spawn_local(async move {
                                                                let _ = ExecApprovalApi::resolve(&dashboard, id.clone(), "deny").await;
                                                                dashboard.pending_approvals.update(|l| l.retain(|x| x.id != id));
                                                            });
                                                        }
                                                    >
                                                        {t!(i18n, notifications.approval_deny)}
                                                    </button>
                                                </div>
                                            </li>
                                        }
                                    }).collect::<Vec<_>>()}
                                </ul>
                            }.into_any()
                        }
                    }}
```

> `dashboard` 是 `DashboardState`（Copy），可被多个 on:click move 闭包捕获，无需 clone。`use_i18n()` 在 map 闭包内取（与本文件既有用法一致）。

- [ ] **Step 4: WASM 构建验证**

Run: `just wasm 2>&1 | tail -30`
Expected: 构建成功，无 dead_code 警告残留（信号/api 均已消费）。若报 `t!` 键缺失 → 检查 Task 3 键名是否一致。

- [ ] **Step 5: 提交（显式路径）**

```bash
git add interfaces/webchat/src/components/notification_center.rs
git commit -m "panel: operator approval cards in notification center"
```

---

## 最终验证（全任务完成后）

- [ ] `just wasm` 绿（Panel dist 重建，无 `t!`/类型/dead_code 错误）
- [ ] 零后端改动确认：`git diff <base>..HEAD --stat` 只含 `interfaces/webchat/` 下文件 + docs
- [ ] 派 final code reviewer 审整体（spec 合规 + 代码质量 + 端到端：approval.* 事件→重拉 pending→卡片→resolve→刷新清除）

## 部署（用户决定时机）

Panel 见效需 `just wasm` → 重编 `aleph-server`（rust_embed 烧 dist）→ 热替换 daemon。3b-1 + 3b-2a 可统一部署。
