# Team Chat UI (Roster Bar + Task Strip) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface live team execution state in the chat window — an always-on top member status bar (collapsing to an avatar cluster when narrow/crowded) and a bottom task strip showing the most-salient team task with a tap-to-open task drawer.

**Architecture:** Pure Panel (Leptos/WASM) re-layout under `interfaces/webchat/`. Zero backend change: reuse the existing `chat.team_members` roster (already live-updated by `team_events.rs` `.activity` branch), the existing `team.*` topic subscription, the existing `teams.list_tasks` RPC, and the existing `CoordTaskDto`. New per-message/task data flows through ONE new local event branch and ONE new ChatState signal. Macro-pattern mirrors the existing `team_participants.rs` floating no-drag affordance and the `--composer-clearance` ResizeObserver.

**Tech Stack:** Rust + Leptos 0.7 (`view!` macro, `RwSignal`, `Effect`, `Memo`, `Show`, context), Tailwind v4 (raw CSS in `interfaces/webchat/styles/tailwind.css`, including a CSS container query), `web_sys::ResizeObserver`, `spawn_local`.

## Global Constraints

- **Scope:** all edits under `interfaces/webchat/`. No edits to `src/` (Rust core), no new RPC/event/enum/DTO. (Spec §0, §9.)
- **Team mode only:** every new affordance renders only when `chat.team_id.get().is_some()`. Single-agent chat must be byte-for-byte unaffected.
- **Reuse 4 member states:** `MemberStatus = { Idle, Working, Done, Error }` (`views/chat/state.rs:240-247`). No "Reviewing" state. (Spec §2.)
- **Reuse existing DTO:** `crate::api::teams::CoordTaskDto` (`api/teams.rs:206-230`) — fields `id, team_id, subject, description, status, owner, priority, result, dependencies, created_at, started_at, completed_at`. **No `updated_at` exists** — recency key is `completed_at → started_at → created_at`. (Spec §3.2 H1.)
- **Task event topic is 4-segment** `team.<id>.task.<verb>` (verb ∈ created/updated/completed/failed/cancelled), payload `{task_id, team_id, status, owner, priority, timestamp}` — **no `subject`**. Match with `topic.contains(".task.")`, never `ends_with(".task")`. (Spec §3.2 H2/M1; mirror `views/teams/kanban.rs:63`.)
- **macOS drag band:** `.aleph-main-drag-band` is `position:absolute; top:0; height:var(--aleph-band-h)` (0px web / 30px macOS), `-webkit-app-region:drag` only on `html[data-platform="macos"]`. Top affordances must carry `aleph-no-drag` + `data-tauri-drag-region="false"` and `z-[60]` (matching `.aleph-sidebar-toggle`). (Spec §1.)
- **tailwind.css comment hazard:** never put `*/` inside a CSS comment (e.g. a glob like `p-*/`) — it terminates the comment early and breaks the build. (Spec §7.)
- **`$WEBCHAT_PKG`** — confirm the webchat crate package name once before running test commands: `grep '^name' interfaces/webchat/Cargo.toml`. Used in all `cargo test -p $WEBCHAT_PKG` commands below.
- **cargo discipline:** prefer `cargo test -p $WEBCHAT_PKG --lib <filter>` for pure-fn tasks; reserve compile gating to `cargo check -p $WEBCHAT_PKG` and run it at most once per UI task. One final `just wasm` for visual verification at the very end.

---

### Task 1: Roster pure logic — `member_status_label` + `collapse_for_count`

Pure, host-testable helpers for the roster bar, co-located with the existing roster helpers (`status_color`, `member_glyph`, `cluster_overflow`) in `team_participants.rs`.

**Files:**
- Modify: `interfaces/webchat/src/components/team_participants.rs` (add two pub fns + tests)

**Interfaces:**
- Produces:
  - `pub fn member_status_label(s: MemberStatus) -> &'static str` — Chinese status word.
  - `pub fn collapse_for_count(n: usize) -> bool` — true when the roster must collapse to the avatar cluster purely on member count (`n > CLUSTER_CAP`).

- [ ] **Step 1: Write the failing tests** — append inside the existing `mod tests` in `team_participants.rs` (before its closing `}`):

```rust
    #[test]
    fn member_status_label_maps_all_variants() {
        assert_eq!(member_status_label(MemberStatus::Working), "工作中");
        assert_eq!(member_status_label(MemberStatus::Idle), "空闲");
        assert_eq!(member_status_label(MemberStatus::Done), "完成");
        assert_eq!(member_status_label(MemberStatus::Error), "错误");
    }

    #[test]
    fn collapse_only_above_cluster_cap() {
        assert!(!collapse_for_count(0));
        assert!(!collapse_for_count(4)); // == CLUSTER_CAP, still expandable
        assert!(collapse_for_count(5)); // first count that forces collapse
        assert!(collapse_for_count(9));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p $WEBCHAT_PKG --lib team_participants`
Expected: FAIL — `cannot find function member_status_label` / `collapse_for_count`.

- [ ] **Step 3: Write the minimal implementation** — add after the existing `status_color` fn in `team_participants.rs`:

```rust
/// Chinese status word shown beside a member's name in the expanded roster bar.
/// Reuses the 4 existing `MemberStatus` variants (no "审阅中" state — spec §2).
#[must_use]
pub fn member_status_label(s: MemberStatus) -> &'static str {
    match s {
        MemberStatus::Working => "工作中",
        MemberStatus::Idle => "空闲",
        MemberStatus::Done => "完成",
        MemberStatus::Error => "错误",
    }
}

/// Count-driven collapse: more than `CLUSTER_CAP` members always render as the
/// avatar cluster (narrow-width collapse is handled separately by a CSS
/// container query). Mirrors the cluster's own `CLUSTER_CAP` cutoff.
#[must_use]
pub fn collapse_for_count(n: usize) -> bool {
    n > CLUSTER_CAP
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p $WEBCHAT_PKG --lib team_participants`
Expected: PASS (all existing + 2 new tests).

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/components/team_participants.rs
git commit -m "panel: add roster status-label + count-collapse pure helpers"
```

---

### Task 2: `TeamParticipants` → responsive roster bar (expanded pills ↔ collapsed cluster)

Evolve the existing component: render an **expanded horizontal pill bar** (when `member_count <= CLUSTER_CAP`) and the **collapsed avatar cluster**, toggled by a CSS container query on width; localize the leader marker to a 「队长」chip; show each member's status dot + Chinese label. Add a `ResizeObserver` publishing the bar's rendered height to `--aleph-team-roster-h`, and pad the message list to clear it. Keep the component name `TeamParticipants` (no import churn).

**Files:**
- Modify: `interfaces/webchat/src/components/team_participants.rs` (rebuild the `view!`)
- Modify: `interfaces/webchat/src/views/chat/view.rs:190-197` (full-width wrapper)
- Modify: `interfaces/webchat/src/views/chat/messages.rs:192-196` (team-mode top padding)
- Modify: `interfaces/webchat/styles/tailwind.css` (roster wrapper + container query + pill styles)

**Interfaces:**
- Consumes: `member_status_label`, `collapse_for_count`, `status_color`, `member_glyph`, `cluster_overflow` (Task 1 + existing); `agent_color_for_id` (existing); `chat.team_members` (existing).
- Produces: CSS var `--aleph-team-roster-h` on `<html>` (bar height in px, `0px` when not in team mode); CSS classes `aleph-roster-wrap`, `aleph-roster-expanded`, `aleph-roster-collapsed`.

- [ ] **Step 1: Add the CSS** — append to `interfaces/webchat/styles/tailwind.css` (raw CSS region, near `.aleph-main-drag-band`). No `*/` inside any comment:

```css
/* Team roster bar wrapper: full-width no-drag float over the top chrome.
   container-type lets the inner expanded/collapsed views swap by width. */
.aleph-roster-wrap {
  container-type: inline-size;
  padding: 6px 8px;
}
/* macOS: clear the traffic lights + sidebar toggle (left:72px) and the
   top-right chrome (workspace toggle + notification bell). */
html[data-platform="macos"] .aleph-roster-wrap {
  padding-left: 84px;
  padding-right: 96px;
}
/* Width swap: default (narrow) shows the cluster; >=560px shows the pill bar.
   560px ~= CLUSTER_CAP(4) * ~140px per pill. */
.aleph-roster-expanded { display: none; }
.aleph-roster-collapsed { display: flex; }
@container (min-width: 560px) {
  .aleph-roster-expanded { display: flex; }
  .aleph-roster-collapsed { display: none; }
}
/* Crowded (> CLUSTER_CAP members): always the cluster, regardless of width.
   Two-class selector outspecifies the bare container-query rules, so it wins
   without !important. The expanded bar is also not rendered (Show=false) when
   crowded; this guarantees the cluster stays visible even at wide widths. */
.aleph-roster-crowded .aleph-roster-expanded { display: none; }
.aleph-roster-crowded .aleph-roster-collapsed { display: flex; }
.aleph-roster-pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px 3px 4px;
  border-radius: 9999px;
  background: color-mix(in srgb, var(--color-surface-raised) 70%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-border) 60%, transparent);
  white-space: nowrap;
}
```

- [ ] **Step 2: Rebuild the component `view!`** — replace the whole `view! { ... }` body of `TeamParticipants` (team_participants.rs:57-154) with the responsive version. Imports at top of file gain `use leptos::wasm_bindgen::JsCast;` (for the ResizeObserver closure) — confirm/add it:

```rust
    let chat = expect_context::<ChatState>();
    let open = RwSignal::new(false);
    let root_ref = NodeRef::<leptos::html::Div>::new();

    // Publish the bar's rendered height to `--aleph-team-roster-h` so the
    // message list can pad its top and not hide the first bubbles behind the
    // floating bar. Mirrors the composer's `--composer-clearance` observer.
    Effect::new(move |_| {
        let Some(el) = root_ref.get() else { return };
        let cb: Closure<dyn FnMut(js_sys::Array)> = Closure::new(move |entries: js_sys::Array| {
            if let Ok(entry) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>() {
                let target: web_sys::Element = entry.target();
                let h = target.get_bounding_client_rect().height();
                if let Some(root) = web_sys::window()
                    .and_then(|w| w.document())
                    .and_then(|d| d.document_element())
                    .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = root
                        .style()
                        .set_property("--aleph-team-roster-h", &format!("{h}px"));
                }
            }
        });
        if let Ok(observer) = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) {
            observer.observe(&el);
        }
        cb.forget();
    });
    // Reset the var when leaving team mode so single chat keeps its normal pad.
    on_cleanup(move || {
        if let Some(root) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = root.style().set_property("--aleph-team-roster-h", "0px");
        }
    });

    view! {
        <div
            node_ref=root_ref
            class="aleph-roster-wrap"
            class:aleph-roster-crowded=move || collapse_for_count(chat.team_members.get().len())
        >
            // Expanded pill bar — one labeled capsule per member. Hidden by the
            // container query when narrow; not rendered at all when crowded.
            <Show when=move || !collapse_for_count(chat.team_members.get().len())>
                <div class="aleph-roster-expanded items-center gap-1.5 overflow-x-auto">
                    {move || {
                        chat.team_members
                            .get()
                            .into_iter()
                            .map(|m| {
                                let color = agent_color_for_id(&m.agent_id);
                                let dot = status_color(m.status);
                                let label = member_status_label(m.status);
                                let glyph = member_glyph(&m);
                                view! {
                                    <span class="aleph-roster-pill">
                                        <span
                                            class="w-6 h-6 rounded-full flex items-center \
                                                   justify-center text-[10px] font-bold \
                                                   text-white shrink-0"
                                            style=format!("background-color: {color};")
                                        >
                                            {glyph}
                                        </span>
                                        <span class="text-xs font-semibold">{m.name}</span>
                                        {m.is_leader.then(|| view! {
                                            <span class="text-[10px] px-1 rounded \
                                                         bg-primary/15 text-primary">"队长"</span>
                                        })}
                                        <span class="text-[10px]" style=format!("color: {dot};")>"●"</span>
                                        <span class="text-[10px] opacity-60">{label}</span>
                                    </span>
                                }
                            })
                            .collect::<Vec<_>>()
                    }}
                </div>
            </Show>

            // Collapsed cluster — always rendered (sole view when crowded; the
            // container query shows it when narrow). Click expands the popover.
            <div class="aleph-roster-collapsed relative">
                <button
                    type="button"
                    class="flex items-center gap-1 rounded-full px-1.5 py-1 \
                           bg-surface-raised/70 backdrop-blur border border-border/60 \
                           hover:bg-surface-raised/90 transition-colors"
                    on:click=move |_| open.update(|o| *o = !*o)
                >
                    <div class="flex items-center">
                        {move || {
                            let members = chat.team_members.get();
                            let mut discs = members
                                .iter()
                                .take(CLUSTER_CAP)
                                .enumerate()
                                .map(|(i, m)| {
                                    let color = agent_color_for_id(&m.agent_id);
                                    let glyph = member_glyph(m);
                                    let margin = if i == 0 { "" } else { "-ml-2" };
                                    view! {
                                        <span
                                            class=format!(
                                                "{margin} w-6 h-6 rounded-full flex items-center \
                                                 justify-center text-[10px] font-bold text-white \
                                                 ring-2 ring-surface-sunken"
                                            )
                                            style=format!("background-color: {color};")
                                        >
                                            {glyph}
                                        </span>
                                    }
                                    .into_any()
                                })
                                .collect::<Vec<_>>();
                            if let Some(extra) = cluster_overflow(members.len()) {
                                discs.push(
                                    view! {
                                        <span
                                            class="-ml-2 w-6 h-6 rounded-full flex items-center \
                                                   justify-center text-[10px] font-bold text-white \
                                                   ring-2 ring-surface-sunken"
                                            style=format!("background-color: {MUTED_GREY};")
                                        >
                                            {format!("+{extra}")}
                                        </span>
                                    }
                                    .into_any(),
                                );
                            }
                            discs
                        }}
                    </div>
                    <span class="text-[10px] opacity-60 ml-0.5">"▾"</span>
                </button>

                // Expanded popover — backdrop catcher + roster card (per-member
                // status dot + Chinese label + 队长 marker).
                <Show when=move || open.get()>
                    <div class="fixed inset-0 z-10" on:click=move |_| open.set(false)></div>
                    <div class="absolute left-0 top-full mt-1 z-20 min-w-[180px] \
                                rounded-lg border border-border bg-surface-raised/95 \
                                backdrop-blur shadow-lg p-1.5 space-y-0.5">
                        {move || {
                            chat.team_members
                                .get()
                                .into_iter()
                                .map(|m| {
                                    let color = agent_color_for_id(&m.agent_id);
                                    let dot = status_color(m.status);
                                    let label = member_status_label(m.status);
                                    let glyph = member_glyph(&m);
                                    view! {
                                        <div class="flex items-center gap-2 text-xs px-1.5 py-1 rounded">
                                            <span style=format!("color: {dot};")>"●"</span>
                                            <span
                                                class="w-6 h-6 rounded-full flex items-center \
                                                       justify-center text-[10px] font-bold \
                                                       text-white shrink-0"
                                                style=format!("background-color: {color};")
                                            >
                                                {glyph}
                                            </span>
                                            {m.is_leader.then(|| view! {
                                                <span class="text-[10px] px-1 rounded \
                                                             bg-primary/15 text-primary">"队长"</span>
                                            })}
                                            <span class="truncate">{m.name}</span>
                                            <span class="text-[10px] opacity-60 ml-auto">{label}</span>
                                        </div>
                                    }
                                })
                                .collect::<Vec<_>>()
                        }}
                    </div>
                </Show>
            </div>
        </div>
    }
```

- [ ] **Step 3: Make the view.rs wrapper full-width** — replace the `<Show>` block at `view.rs:190-197` with a full-width top-0 wrapper (drop `top-2 left-2`; the `aleph-roster-wrap` class owns padding):

```rust
                    <Show when=move || chat.team_id.get().is_some()>
                        <div
                            class="absolute top-0 inset-x-0 z-[60] aleph-no-drag"
                            data-tauri-drag-region="false"
                        >
                            <TeamParticipants />
                        </div>
                    </Show>
```

- [ ] **Step 4: Pad the message list in team mode** — in `messages.rs`, replace the `fallback`'s outer `class=move || format!(...)` (lines 192-196) so team mode pads for the roster bar:

```rust
                        <div class=move || {
                            let top = if chat.team_id.get().is_some() {
                                // Roster bar floats over the top; clear its
                                // measured height (fallback ~2.75rem pre-observe).
                                "pt-[calc(var(--aleph-team-roster-h,2.75rem)+0.75rem)]".to_string()
                            } else if sessions.tab_strip_visible() {
                                // pt-14 = band (~33px) + headroom
                                "pt-14".to_string()
                            } else {
                                "pt-6".to_string()
                            };
                            format!(
                                "max-w-3xl mx-auto px-4 {top} \
                                 pb-[calc(var(--composer-clearance,150px)+1rem)] space-y-2"
                            )
                        }>
```

- [ ] **Step 5: Compile-check**

Run: `cargo check -p $WEBCHAT_PKG`
Expected: clean compile (no errors). If `JsCast`/`Closure` unresolved, add `use leptos::wasm_bindgen::prelude::Closure;` and `use leptos::wasm_bindgen::JsCast;` at the top of `team_participants.rs` (match the imports used in `composer/mod.rs`).

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/components/team_participants.rs \
        interfaces/webchat/src/views/chat/view.rs \
        interfaces/webchat/src/views/chat/messages.rs \
        interfaces/webchat/styles/tailwind.css
git commit -m "panel: responsive team roster bar (pills <-> cluster) with 队长 chip"
```

---

### Task 3: `ChatState.team_tasks` signal + task pure logic

Add the task signal and the host-testable task helpers used by the strip and drawer.

**Files:**
- Modify: `interfaces/webchat/src/views/chat/state.rs` (add field + init + `use`)
- Create: `interfaces/webchat/src/views/chat/team_task_logic.rs` (pure fns + tests)
- Modify: `interfaces/webchat/src/views/chat/mod.rs` (register module)

**Interfaces:**
- Produces:
  - `chat.team_tasks: RwSignal<Vec<CoordTaskDto>>` on `ChatState`.
  - `pub fn task_status_label(status: &str) -> String` (echoes unknown variants verbatim)
  - `pub fn task_status_color(status: &str) -> &'static str`
  - `pub fn most_salient_task(tasks: &[CoordTaskDto]) -> Option<&CoordTaskDto>`
  - `pub fn extra_task_count(total: usize) -> Option<usize>`

- [ ] **Step 1: Write the failing tests** — create `interfaces/webchat/src/views/chat/team_task_logic.rs`:

```rust
//! Host-testable pure logic for the team task strip/drawer: status → label/color
//! mapping over the raw snake_case `CoordTaskStatus` wire strings, "most salient"
//! task selection, and the overflow count. No Leptos signals / DOM here.

use crate::api::teams::CoordTaskDto;

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, status: &str, created: u64, started: Option<u64>, completed: Option<u64>) -> CoordTaskDto {
        CoordTaskDto {
            id: id.to_string(),
            team_id: Some("t1".to_string()),
            subject: format!("subj-{id}"),
            description: String::new(),
            status: status.to_string(),
            owner: None,
            priority: "normal".to_string(),
            result: None,
            dependencies: Vec::new(),
            created_at: created,
            started_at: started,
            completed_at: completed,
        }
    }

    #[test]
    fn label_maps_all_ten_variants_and_echoes_unknown() {
        assert_eq!(task_status_label("waiting_review"), "待审阅");
        assert_eq!(task_status_label("in_progress"), "进行中");
        assert_eq!(task_status_label("pending"), "待处理");
        assert_eq!(task_status_label("blocked"), "阻塞");
        assert_eq!(task_status_label("completed"), "已完成");
        assert_eq!(task_status_label("failed"), "失败");
        assert_eq!(task_status_label("cancelled"), "已取消");
        assert_eq!(task_status_label("skipped"), "已跳过");
        assert_eq!(task_status_label("paused"), "已暂停");
        assert_eq!(task_status_label("unsatisfiable"), "不可满足");
        // Unknown / future variants echo verbatim (never panics, forward-compatible).
        assert_eq!(task_status_label("weird_future_state"), "weird_future_state");
    }

    #[test]
    fn salient_prefers_waiting_review_over_in_progress() {
        let tasks = vec![
            task("a", "in_progress", 10, Some(11), None),
            task("b", "waiting_review", 5, Some(6), None),
        ];
        assert_eq!(most_salient_task(&tasks).unwrap().id, "b");
    }

    #[test]
    fn salient_breaks_ties_by_recency_then_id() {
        // Both in_progress; pick the most-recently-advanced (started_at), then id.
        let tasks = vec![
            task("a", "in_progress", 1, Some(20), None),
            task("b", "in_progress", 1, Some(50), None),
            task("c", "pending", 1, None, None),
        ];
        assert_eq!(most_salient_task(&tasks).unwrap().id, "b");
    }

    #[test]
    fn salient_none_for_empty() {
        assert!(most_salient_task(&[]).is_none());
    }

    #[test]
    fn extra_count_hides_at_zero_and_one() {
        assert_eq!(extra_task_count(0), None);
        assert_eq!(extra_task_count(1), None);
        assert_eq!(extra_task_count(2), Some(1));
        assert_eq!(extra_task_count(5), Some(4));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p $WEBCHAT_PKG --lib team_task_logic`
Expected: FAIL — module/functions not found (and `mod team_task_logic;` not yet registered → compile error). Proceed to implement.

- [ ] **Step 3: Implement the pure fns** — prepend to `team_task_logic.rs` (above the `#[cfg(test)]` block):

```rust
/// Chinese label for a raw `CoordTaskStatus` wire string (snake_case, all 10
/// variants from src/agents/swarm/tasks/mod.rs). Unknown / future variants echo
/// verbatim so the strip/drawer always render something (never panics).
#[must_use]
pub fn task_status_label(status: &str) -> String {
    match status {
        "waiting_review" => "待审阅",
        "in_progress" => "进行中",
        "pending" => "待处理",
        "blocked" => "阻塞",
        "completed" => "已完成",
        "failed" => "失败",
        "cancelled" => "已取消",
        "skipped" => "已跳过",
        "paused" => "已暂停",
        "unsatisfiable" => "不可满足",
        other => return other.to_string(),
    }
    .to_string()
}

/// Status dot color for a task (CSS hex), reusing the member palette family.
#[must_use]
pub fn task_status_color(status: &str) -> &'static str {
    match status {
        "waiting_review" => "#c586c0", // purple — needs attention
        "in_progress" => "#e0a458",    // amber — active
        "completed" | "skipped" => "#4ec9b0", // teal — done
        "failed" | "unsatisfiable" => "#d16969", // red — bad terminal
        _ => "#6b7280",                // grey — pending/blocked/paused/unknown
    }
}

/// Lower rank = more salient. WaitingReview > InProgress > other non-terminal >
/// terminal. (Spec §3.2.)
fn salience_rank(status: &str) -> u8 {
    match status {
        "waiting_review" => 0,
        "in_progress" => 1,
        "completed" | "failed" | "cancelled" | "skipped" | "unsatisfiable" => 3,
        _ => 2, // pending / blocked / paused / unknown — non-terminal
    }
}

/// Recency key from existing timestamps (no `updated_at` field exists).
fn recency_key(t: &CoordTaskDto) -> u64 {
    t.completed_at.or(t.started_at).unwrap_or(t.created_at)
}

/// The single most-attention-worthy task: lowest salience rank, then most
/// recent, then lowest id (deterministic). `None` for an empty list.
#[must_use]
pub fn most_salient_task(tasks: &[CoordTaskDto]) -> Option<&CoordTaskDto> {
    tasks.iter().min_by(|a, b| {
        salience_rank(&a.status)
            .cmp(&salience_rank(&b.status))
            .then(recency_key(b).cmp(&recency_key(a))) // newer first
            .then(a.id.cmp(&b.id))
    })
}

/// "+N" badge value = remaining tasks after the salient one. `None` when ≤1.
#[must_use]
pub fn extra_task_count(total: usize) -> Option<usize> {
    total.checked_sub(1).filter(|&n| n > 0)
}
```

- [ ] **Step 4: Register the module** — in `interfaces/webchat/src/views/chat/mod.rs`, add alongside the other `pub mod` lines (e.g. near `pub mod team_events;`). It MUST be `pub mod` (matching every sibling) because Task 5's strip lives in `components/` (outside `views::chat`) and imports `crate::views::chat::team_task_logic::…` — a private module isn't reachable cross-tree:

```rust
pub mod team_task_logic;
```

- [ ] **Step 5: Add the ChatState field** — in `state.rs`, add the `use` near the top (with the other `crate::api` imports):

```rust
use crate::api::teams::CoordTaskDto;
```

In the `ChatState` struct (after `pub messages: RwSignal<Vec<ChatMessage>>,` or near the other team signals), add:

```rust
    /// Team chat: coordination tasks for the active team (drives the bottom
    /// task strip + drawer). Empty when not in team mode. Fetched from
    /// `teams.list_tasks` and upserted by `team.<id>.task.<verb>` events.
    pub team_tasks: RwSignal<Vec<CoordTaskDto>>,
```

Then find where `ChatState` is constructed and initialize the field. Locate it:

Run: `grep -n "team_members: RwSignal::new" interfaces/webchat/src/views/chat/state.rs`

At that construction site, add next to `team_members`:

```rust
            team_tasks: RwSignal::new(Vec::new()),
```

- [ ] **Step 6: Run tests + compile**

Run: `cargo test -p $WEBCHAT_PKG --lib team_task_logic`
Expected: PASS (all 5 tests).
Run: `cargo check -p $WEBCHAT_PKG`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/views/chat/team_task_logic.rs \
        interfaces/webchat/src/views/chat/mod.rs \
        interfaces/webchat/src/views/chat/state.rs
git commit -m "panel: add chat.team_tasks signal + task status/salience pure logic"
```

---

### Task 4: Task data flow — initial fetch + incremental `.task.` events

Populate `chat.team_tasks` from `teams.list_tasks` when entering team mode, and keep it live by handling `team.<id>.task.<verb>` events in `team_events.rs` (upsert by `task_id`; refetch when an unknown id appears, since the event payload has no `subject`).

**Files:**
- Modify: `interfaces/webchat/src/views/chat/team_events.rs` (capture `DashboardState`; add `.task.` branch)
- Modify: `interfaces/webchat/src/views/chat/view.rs` (initial fetch Effect)

**Interfaces:**
- Consumes: `chat.team_tasks` (Task 3), `chat.team_id` (existing), `crate::api::teams::{TeamsApi, TaskFilter, CoordTaskDto}` (existing).
- Produces: `chat.team_tasks` kept in sync with backend.

- [ ] **Step 1: Capture `DashboardState` for refetch** — in `team_events.rs`, change `subscribe_team_events` to copy the dashboard handle into the closure. Update the top `use` and signature body:

Add to imports:

```rust
use crate::api::teams::{CoordTaskDto, TaskFilter, TeamsApi};
```

At the start of `subscribe_team_events`, copy the handle (DashboardState is `Copy`, like `ChatState`):

```rust
pub fn subscribe_team_events(dashboard: &DashboardState, chat: ChatState) -> usize {
    let dash = *dashboard;
    dashboard.subscribe_events(move |event: GatewayEvent| {
```

- [ ] **Step 2: Add the `.task.` branch** — in the same closure, after the existing `} else if event.topic.ends_with(".activity") { ... }` block (team_events.rs:59-71) and before its closing, add:

```rust
        } else if event.topic.contains(".task.") {
            // 4-segment topic `team.<id>.task.<verb>`; payload carries
            // {task_id, status, ...} but NO subject. Upsert status in place for
            // known tasks; refetch the list for an unknown id (new task needs
            // its subject). Idempotent + order-independent: unknown/terminal/
            // deleted ids are tolerated.
            let task_id = data
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if task_id.is_empty() {
                return;
            }
            let status = data
                .get("status")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let known = chat
                .team_tasks
                .with_untracked(|ts| ts.iter().any(|t| t.id == task_id));
            if known {
                chat.team_tasks.update(|ts| {
                    if let Some(t) = ts.iter_mut().find(|t| t.id == task_id) {
                        if let Some(s) = status {
                            t.status = s;
                        }
                    }
                });
            } else if let Some(team_id) = chat.team_id.get_untracked() {
                let chat2 = chat;
                spawn_local(async move {
                    if let Ok(tasks) =
                        TeamsApi::list_tasks(&dash, &team_id, TaskFilter::default()).await
                    {
                        chat2.team_tasks.set(tasks);
                    }
                });
            }
```

Note: this branch references `spawn_local` and `CoordTaskDto`/`TaskFilter`/`TeamsApi`. Ensure `use leptos::task::spawn_local;` (or the crate's existing `spawn_local` path — match `view.rs`) is imported in `team_events.rs`.

- [ ] **Step 3: Initial fetch on entering team mode** — in `view.rs`, after the existing `let team_sub_id = subscribe_team_events(&dashboard, chat);` (view.rs:38), add an Effect that fetches the task list once the session is connected and a team is active:

```rust
    // Team chat: hydrate the task strip/drawer from teams.list_tasks whenever
    // the active team changes (and we're connected). Incremental updates after
    // this come from the team.<id>.task.<verb> branch in team_events.rs.
    let dash_for_tasks = dashboard;
    Effect::new(move |_| {
        let Some(team_id) = chat.team_id.get() else {
            chat.team_tasks.set(Vec::new());
            return;
        };
        if !dash_for_tasks.is_connected.get() {
            return;
        }
        let chat2 = chat;
        spawn_local(async move {
            if let Ok(tasks) = crate::api::teams::TeamsApi::list_tasks(
                &dash_for_tasks,
                &team_id,
                crate::api::teams::TaskFilter::default(),
            )
            .await
            {
                chat2.team_tasks.set(tasks);
            }
        });
    });
```

- [ ] **Step 4: Compile-check**

Run: `cargo check -p $WEBCHAT_PKG`
Expected: clean. (No unit test here — the branch lives inside an event closure; its core selection logic is already covered by Task 3's `most_salient_task` tests.)

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/chat/team_events.rs \
        interfaces/webchat/src/views/chat/view.rs
git commit -m "panel: hydrate + live-update chat.team_tasks (fetch + .task. events)"
```

---

### Task 5: `TeamTaskStrip` component + mount in composer

Render the most-salient task as a single tappable pill above the input box; hidden when there are no tasks.

**Files:**
- Create: `interfaces/webchat/src/components/team_task_strip.rs`
- Modify: `interfaces/webchat/src/components/mod.rs` (register + re-export)
- Modify: `interfaces/webchat/src/views/chat/composer/mod.rs:683-686` (mount in the stack)
- Modify: `interfaces/webchat/src/views/chat/view.rs` (provide `TaskDrawerOpen` context in `ChatView`)
- Modify: `interfaces/webchat/styles/tailwind.css` (strip style)

**Interfaces:**
- Consumes: `chat.team_tasks`, `chat.team_id` (Task 3/4); `most_salient_task`, `task_status_label`, `task_status_color`, `extra_task_count` (Task 3).
- Produces: `#[component] pub fn TeamTaskStrip()`; the `TaskDrawerOpen(RwSignal<bool>)` newtype (defined here; **provided by `ChatView` in Step 4**; consumed by the strip and, in Task 6, the drawer).

- [ ] **Step 1: Create the component** — `interfaces/webchat/src/components/team_task_strip.rs`:

```rust
//! Bottom task strip for team chat: one tappable pill showing the most-salient
//! team task (`● 任务 · {subject} · {状态}  +N`). Hidden when the team has no
//! tasks. Lives in the composer's floating stack so it sits just above the
//! input box and is covered by the same `--composer-clearance` measurement.
//! Tapping toggles `TaskDrawerOpen` (consumed by `TeamTaskDrawer`).

use leptos::prelude::*;

use crate::views::chat::state::ChatState;
use crate::views::chat::team_task_logic::{
    extra_task_count, most_salient_task, task_status_color, task_status_label,
};

/// Shared open-state for the team task drawer (set by the strip, read by the
/// drawer). Provided by `ChatView` (view.rs) — a common ancestor of both the
/// strip and the drawer — so `expect_context` resolves in both subtrees.
#[derive(Clone, Copy)]
pub struct TaskDrawerOpen(pub RwSignal<bool>);

#[component]
#[must_use]
pub fn TeamTaskStrip() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    // Open-state lives in ChatView's context (Step 4) so the sibling
    // TeamTaskDrawer reads the same signal. The strip only sets it.
    let TaskDrawerOpen(drawer_open) = expect_context::<TaskDrawerOpen>();

    view! {
        <Show when=move || {
            chat.team_id.get().is_some() && !chat.team_tasks.get().is_empty()
        }>
            <button
                type="button"
                class="w-full mb-2 flex items-center gap-2 px-3 py-1.5 rounded-full \
                       text-xs bg-surface-raised/70 backdrop-blur border border-border/60 \
                       hover:bg-surface-raised/90 transition-colors text-left"
                on:click=move |_| drawer_open.set(true)
            >
                {move || {
                    let tasks = chat.team_tasks.get();
                    let Some(top) = most_salient_task(&tasks) else {
                        return view! { <span></span> }.into_any();
                    };
                    let dot = task_status_color(&top.status);
                    let label = task_status_label(&top.status);
                    let subject = top.subject.clone();
                    let extra = extra_task_count(tasks.len());
                    view! {
                        <span style=format!("color: {dot};")>"●"</span>
                        <span class="opacity-60">"任务"</span>
                        <span class="opacity-40">"·"</span>
                        <span class="font-medium truncate">{subject}</span>
                        <span class="opacity-40">"·"</span>
                        <span class="opacity-70">{label}</span>
                        {extra.map(|n| view! {
                            <span class="ml-auto text-[10px] px-1.5 py-0.5 rounded-full \
                                         bg-border/40 opacity-70">{format!("+{n}")}</span>
                        })}
                    }
                    .into_any()
                }}
            </button>
        </Show>
    }
}
```

- [ ] **Step 2: Register + re-export** — in `interfaces/webchat/src/components/mod.rs`, add next to the other component modules (mirror the `team_participants` lines):

```rust
mod team_task_strip;
pub use team_task_strip::{TaskDrawerOpen, TeamTaskStrip};
```

(If `mod.rs` uses `pub mod` / a different re-export style, match it. Confirm how `TeamParticipants` is exported and copy that exact pattern.)

- [ ] **Step 3: Mount in the composer stack** — in `composer/mod.rs`, insert `<TeamTaskStrip />` into the `stack_ref` div, right after `<QueuedPromptBar .../>` (line 686), so it's measured by the ResizeObserver and floats above the input:

```rust
                <AttachmentPreviewBar attachments=attachments />

                <QueuedPromptBar queue=chat.prompt_queue />

                // Team chat: most-salient task pill, above the input box.
                <TeamTaskStrip />
```

Add the import at the top of `composer/mod.rs` (match the existing component-import style):

```rust
use crate::components::TeamTaskStrip;
```

- [ ] **Step 4: Provide `TaskDrawerOpen` in `ChatView`** — in `view.rs`, in the `ChatView` setup (before the `view! {`, alongside the `expect_context`/`subscribe_*` setup near view.rs:30-38), create the shared open-signal and provide it so both the strip (a composer descendant) and the Task 6 drawer (a chat-column child) resolve the **same** context — siblings can't share a context provided lower down:

```rust
    // Team task drawer open-state — provided at the chat-view level so both the
    // composer's TeamTaskStrip and the chat-column TeamTaskDrawer (Task 6) read
    // the same signal via expect_context.
    let task_drawer_open = RwSignal::new(false);
    provide_context(TaskDrawerOpen(task_drawer_open));
```

Add the import at the top of `view.rs` (match the existing component-import style):

```rust
use crate::components::TaskDrawerOpen;
```

- [ ] **Step 5: Add the strip style (optional polish)** — append to `tailwind.css` (no `*/` in comments). Only if the inline classes need a shared tweak; otherwise skip. Minimal:

```css
/* Team task strip pill sits in the composer stack; truncation keeps it on one
   line regardless of subject length. */
.aleph-task-strip-subject { min-width: 0; }
```

- [ ] **Step 6: Compile-check**

Run: `cargo check -p $WEBCHAT_PKG`
Expected: clean. (`provide_context`/`RwSignal` are already in scope in `view.rs` via `leptos::prelude::*`.)

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/components/team_task_strip.rs \
        interfaces/webchat/src/components/mod.rs \
        interfaces/webchat/src/views/chat/composer/mod.rs \
        interfaces/webchat/src/views/chat/view.rs \
        interfaces/webchat/styles/tailwind.css
git commit -m "panel: bottom team task strip (most-salient task pill) in composer"
```

---

### Task 6: `TeamTaskDrawer` — slide-over task list + wire strip click

A right-side slide-over listing all team tasks (subject + status pill), opened by the strip, closed by a backdrop click. Reuses `chat.team_tasks` (already fetched/live) — no new data wiring.

**Files:**
- Modify: `interfaces/webchat/src/components/team_task_strip.rs` (add `TeamTaskDrawer`)
- Modify: `interfaces/webchat/src/components/mod.rs` (re-export `TeamTaskDrawer`)
- Modify: `interfaces/webchat/src/views/chat/view.rs` (mount drawer in team mode)

**Interfaces:**
- Consumes: `TaskDrawerOpen` context (Task 5); `chat.team_tasks`; `task_status_label`, `task_status_color` (Task 3).
- Produces: `#[component] pub fn TeamTaskDrawer()`.

- [ ] **Step 1: Add the drawer component** — append to `team_task_strip.rs`:

```rust
#[component]
#[must_use]
pub fn TeamTaskDrawer() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let TaskDrawerOpen(open) = expect_context::<TaskDrawerOpen>();

    view! {
        <Show when=move || open.get()>
            // Backdrop catcher — click outside closes.
            <div class="fixed inset-0 z-[80] bg-black/20" on:click=move |_| open.set(false)></div>
            // Slide-over panel.
            <div class="fixed top-0 right-0 bottom-0 z-[81] w-[320px] max-w-[85vw] \
                        bg-surface-raised/95 backdrop-blur border-l border-border \
                        shadow-xl flex flex-col aleph-no-drag"
                 data-tauri-drag-region="false">
                <div class="flex items-center justify-between px-4 py-3 border-b border-border">
                    <span class="text-sm font-semibold">"团队任务"</span>
                    <button
                        type="button"
                        class="text-xs opacity-60 hover:opacity-100"
                        on:click=move |_| open.set(false)
                    >"✕"</button>
                </div>
                <div class="flex-1 overflow-y-auto p-2 space-y-1">
                    {move || {
                        let tasks = chat.team_tasks.get();
                        if tasks.is_empty() {
                            return view! {
                                <div class="text-xs opacity-50 px-2 py-4 text-center">"暂无任务"</div>
                            }.into_any();
                        }
                        tasks
                            .into_iter()
                            .map(|t| {
                                let dot = task_status_color(&t.status);
                                let label = task_status_label(&t.status);
                                view! {
                                    <div class="flex items-center gap-2 px-2 py-2 rounded \
                                                hover:bg-surface-sunken/40 text-xs">
                                        <span style=format!("color: {dot};")>"●"</span>
                                        <span class="flex-1 truncate">{t.subject}</span>
                                        <span class="text-[10px] opacity-60 shrink-0">{label}</span>
                                    </div>
                                }
                            })
                            .collect::<Vec<_>>()
                            .into_any()
                    }}
                </div>
            </div>
        </Show>
    }
}
```

Update the import line at the top of `team_task_strip.rs` to also use the two status fns in the drawer (already imported in Task 5; confirm `task_status_color` + `task_status_label` are in the `use`).

- [ ] **Step 2: Re-export** — in `components/mod.rs`, extend the re-export:

```rust
pub use team_task_strip::{TaskDrawerOpen, TeamTaskDrawer, TeamTaskStrip};
```

- [ ] **Step 3: Mount the drawer** — in `view.rs`, inside the chat-surface column (a sibling of `<InputArea />`, still under the `relative flex-1 min-h-0` div at view.rs:169), add the drawer gated on team mode so the `TaskDrawerOpen` context (provided by `TeamTaskStrip`, which mounts via `InputArea`) is in scope. Place it right after `<InputArea />` (view.rs:199):

```rust
                    <InputArea />
                    <Show when=move || chat.team_id.get().is_some()>
                        <TeamTaskDrawer />
                    </Show>
```

Add the import at the top of `view.rs` (match existing component-import style):

```rust
use crate::components::TeamTaskDrawer;
```

> Context note: `TaskDrawerOpen` is provided by `ChatView` itself (Task 5 Step 4), which is a common ancestor of both `TeamTaskStrip` (deep in the composer) and this `TeamTaskDrawer` (a direct chat-column child). Both resolve the same signal via `expect_context` — no panic. The drawer's `fixed inset-0` overlay sits at the same chat-column depth as the proven `TeamParticipants` popover backdrop, so its full-screen positioning is not trapped by a transformed ancestor.

- [ ] **Step 4: Compile-check**

Run: `cargo check -p $WEBCHAT_PKG`
Expected: clean.

- [ ] **Step 5: Visual + interaction verification (single wasm build)**

Run: `just wasm`
Then run the dev server (`just dev`) or refresh the desktop shell per DESKTOP_SHELL.md, enter a team chat, and confirm:
- Top: member pills with name + status dot + label; leader shows 「队长」; narrowing the window collapses to the avatar cluster; clicking the cluster shows per-member status.
- First message is not hidden behind the bar (top padding correct).
- macOS: the gaps between pills still drag the window; pills/cluster are clickable.
- Bottom: task strip shows the salient task + `+N`; tapping opens the drawer; backdrop closes it.
- Single-agent chat (no team): no roster bar, no task strip, layout unchanged.

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/components/team_task_strip.rs \
        interfaces/webchat/src/components/mod.rs \
        interfaces/webchat/src/views/chat/view.rs
git commit -m "panel: team task drawer (slide-over list) wired to task strip"
```

---

## Self-Review

**Spec coverage:**
- §3.1 roster bar (responsive pills ↔ cluster, 队长 chip, status dot+label, no-drag float, `--aleph-team-roster-h`, message padding) → Tasks 1–2. ✓
- §3.2 task strip (salient task + `+N`, hidden when empty, drawer on click, reuse `CoordTaskDto`, `.contains(".task.")` upsert + refetch) → Tasks 3–6. ✓
- §3.3 status→label/color (member 4-state + task 10-state + unknown fallback, raw snake_case) → Tasks 1, 3. ✓
- §4 session_tabs coexistence (not suppressed) → roster moved to full-width `top-0`; SessionTabs untouched (renders only ≥2 sessions). Rare team+multi-session top overlap is a documented cosmetic follow-up, switch-back affordance intact. ✓ (see Known Limitations)
- §5 data flow (fetch + `.activity` unchanged + new `.task.` branch, global `team.*` sub) → Task 4. ✓
- §6 error handling (empty roster/tasks hidden, monogram fallback, unknown-status passthrough, upsert tolerates unknown/terminal/deleted ids, ResizeObserver reset on leave) → Tasks 2–6. ✓
- §7 tests (pure-fn units + manual UI) → Tasks 1, 3 (units); Tasks 2,5,6 (manual). ✓

**Placeholder scan:** no TBD/TODO; every code step shows full code; the two runtime-uncertainty notes (import path of `spawn_local`; `TaskDrawerOpen` context subtree) include concrete fallbacks, not deferrals. ✓

**Type consistency:** `member_status_label`/`collapse_for_count`/`status_color`/`member_glyph`/`cluster_overflow` (Task 1/existing); `task_status_label`/`task_status_color`/`most_salient_task`/`extra_task_count`/`TaskDrawerOpen` used identically across Tasks 3, 5, 6; `chat.team_tasks: RwSignal<Vec<CoordTaskDto>>` defined Task 3, consumed Tasks 4–6. ✓

## Known Limitations (documented, not deferred work)
- **Narrow-collapse breakpoint** is a fixed `560px` container query (≈4×140px), not a per-pill measured width — simpler than a JS width observer and flicker-free. (Spec §3.1 allowed CSS container query.)
- **Team + concurrent non-team sessions:** if `SessionTabs` (≥2 sessions) and the roster are both visible, they share the top band and may visually overlap. `SessionTabs` stays functional (switch-back preserved); a precise non-overlap stacking is a cosmetic follow-up, out of scope here.
