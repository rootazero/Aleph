# Team Selector Pill Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the Teams-tab team selector to the top of the sidebar and restyle it as a `ModelPicker`-style pill + popover, so switching between chat and teams tabs feels consistent.

**Architecture:** Rewrite the existing `TeamSelector` Leptos component (same name, same file) from a native `<select>` into a pill trigger + floating popover that mirrors `components/model_picker.rs`. Extract two pure helpers (`current_team_label`, `filter_teams`) for host unit testing. Relocate the pill to the top of `TeamsSidebar` and delete the old bottom selector block. The reactive wiring (`selected_team_id` → kanban/plan/replay Effects) is unchanged; this is a UI relocation + restyle only.

**Tech Stack:** Rust, Leptos (CSR/WASM), Tailwind CSS utility classes, `aleph-panel` crate (dir `interfaces/webchat`).

## Global Constraints

- Reply in Chinese; code comments in English (project rule).
- Extreme cargo restraint: at most one host test run + one wasm `cargo check` for the whole plan. Do NOT run full test suites.
- Commit/push ONLY when the user explicitly asks. The "commit" step at the end is gated on user approval — do not commit autonomously.
- No backend / RPC / data-model changes. Frontend only.
- Reference style is `interfaces/webchat/src/components/model_picker.rs` — match its trigger/popover Tailwind classes.
- Status string: a team is active iff `status == "active"`; anything else is disbanded.
- Interactive popover buttons must call `ev.prevent_default()` on `mousedown` to avoid the macOS WKWebView focus/blur bug.

---

### Task 1: Add i18n keys for the popover

**Files:**
- Modify: `interfaces/webchat/locales/en.json` (the `teams.kanban` block, after `selector_placeholder` ~line 1487)
- Modify: `interfaces/webchat/locales/zh.json` (the `teams.kanban` block, after `selector_placeholder` ~line 1487)

**Interfaces:**
- Consumes: nothing.
- Produces: i18n keys `teams.kanban.filter_placeholder`, `teams.kanban.empty_teams`, `teams.kanban.no_match` (used by Task 2's component via `t!`/`t_string!`).

- [ ] **Step 1: Add the three keys in `en.json`**

In `interfaces/webchat/locales/en.json`, the `teams.kanban` block currently begins:

```json
    "kanban": {
      "selector_label": "Active Team",
      "selector_placeholder": "Choose a team",
      "columns": {
```

Change it to:

```json
    "kanban": {
      "selector_label": "Active Team",
      "selector_placeholder": "Choose a team",
      "filter_placeholder": "Filter teams…",
      "empty_teams": "No teams yet",
      "no_match": "No matching team",
      "columns": {
```

- [ ] **Step 2: Add the three keys in `zh.json`**

In `interfaces/webchat/locales/zh.json`, the `teams.kanban` block currently begins:

```json
    "kanban": {
      "selector_label": "当前团队",
      "selector_placeholder": "选择团队",
      "columns": {
```

Change it to:

```json
    "kanban": {
      "selector_label": "当前团队",
      "selector_placeholder": "选择团队",
      "filter_placeholder": "过滤团队…",
      "empty_teams": "暂无团队",
      "no_match": "无匹配团队",
      "columns": {
```

- [ ] **Step 3: Validate JSON**

Run: `python3 -m json.tool interfaces/webchat/locales/en.json >/dev/null && python3 -m json.tool interfaces/webchat/locales/zh.json >/dev/null && echo OK`
Expected: `OK` (both files are valid JSON).

> No commit here — fold into the final gated commit step (Task 4).

---

### Task 2: Rewrite `TeamSelector` as pill + popover with tested helpers

**Files:**
- Modify (full rewrite): `interfaces/webchat/src/views/teams/components/team_selector.rs`
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::api::teams::TeamSummary { id: String, name: String, status: String, .. }`; `crate::views::teams::TeamsTabState { teams: RwSignal<Vec<TeamSummary>>, selected_team_id: RwSignal<Option<String>>, .. }`; i18n keys from Task 1.
- Produces:
  - `pub fn current_team_label(teams: &[TeamSummary], selected_id: Option<&str>, placeholder: &str) -> String`
  - `pub fn filter_teams(teams: &[TeamSummary], query: &str) -> Vec<TeamSummary>`
  - `pub fn TeamSelector() -> impl IntoView` (component, unchanged public name — `mod.rs` import path stays `components::team_selector::TeamSelector`).

- [ ] **Step 1: Write the failing tests (helpers don't exist yet)**

Replace the entire contents of `interfaces/webchat/src/views/teams/components/team_selector.rs` test section by writing the full file as shown in Step 3, but first confirm the tests below are what we assert. The `#[cfg(test)]` module at the bottom of the new file is:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::teams::TeamSummary;

    fn team(id: &str, name: &str, status: &str) -> TeamSummary {
        TeamSummary {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            leader_id: "main".into(),
            status: status.into(),
            member_count: 0,
            task_count: 0,
            created_at: 0,
            disbanded_at: None,
            members_preview: Vec::new(),
            last_message: None,
            last_message_at: None,
        }
    }

    #[test]
    fn label_returns_selected_team_name() {
        let teams = vec![team("a", "Alpha", "active"), team("b", "Beta", "active")];
        assert_eq!(current_team_label(&teams, Some("b"), "Choose"), "Beta");
    }

    #[test]
    fn label_falls_back_to_placeholder_when_none_selected() {
        let teams = vec![team("a", "Alpha", "active")];
        assert_eq!(current_team_label(&teams, None, "Choose"), "Choose");
    }

    #[test]
    fn label_falls_back_to_placeholder_when_id_missing() {
        let teams = vec![team("a", "Alpha", "active")];
        assert_eq!(current_team_label(&teams, Some("gone"), "Choose"), "Choose");
    }

    #[test]
    fn filter_empty_query_returns_all() {
        let teams = vec![team("a", "Alpha", "active"), team("b", "Beta", "active")];
        assert_eq!(filter_teams(&teams, "   ").len(), 2);
    }

    #[test]
    fn filter_is_case_insensitive_substring() {
        let teams = vec![team("a", "Alpha", "active"), team("b", "Beta", "disbanded")];
        let got = filter_teams(&teams, "ET");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "b");
    }

    #[test]
    fn filter_no_match_returns_empty() {
        let teams = vec![team("a", "Alpha", "active")];
        assert!(filter_teams(&teams, "zzz").is_empty());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail to compile (helpers undefined)**

Run: `cargo test -p aleph-panel --lib team_selector 2>&1 | tail -20`
Expected: FAIL — `cannot find function current_team_label` / `filter_teams` (or, if you write the whole file at once in Step 3, skip straight to Step 4).

- [ ] **Step 3: Write the full new file (helpers + component + tests)**

Replace the ENTIRE contents of `interfaces/webchat/src/views/teams/components/team_selector.rs` with:

```rust
//! `TeamSelector` — pill + popover bound to `TeamsTabState.selected_team_id`,
//! mirroring the chat composer's `ModelPicker` (`components/model_picker.rs`)
//! so switching between the chat and teams tabs feels consistent.

use crate::api::teams::TeamSummary;
use crate::i18n::{t, t_string, use_i18n};
use crate::views::teams::TeamsTabState;
use leptos::prelude::*;

/// Pill label: the selected team's name, or `placeholder` when nothing is
/// selected or the selected id is no longer present in `teams`.
pub fn current_team_label(
    teams: &[TeamSummary],
    selected_id: Option<&str>,
    placeholder: &str,
) -> String {
    selected_id
        .and_then(|id| teams.iter().find(|t| t.id == id))
        .map(|t| t.name.clone())
        .unwrap_or_else(|| placeholder.to_string())
}

/// Order-preserving, case-insensitive substring filter over team names.
/// An empty/whitespace query returns every team (clone of the input).
pub fn filter_teams(teams: &[TeamSummary], query: &str) -> Vec<TeamSummary> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return teams.to_vec();
    }
    teams
        .iter()
        .filter(|t| t.name.to_lowercase().contains(&q))
        .cloned()
        .collect()
}

#[component]
#[must_use]
pub fn TeamSelector() -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<TeamsTabState>();
    let open = RwSignal::new(false);
    let search = RwSignal::new(String::new());

    // Reset the filter whenever the popover closes (mirrors ModelPicker).
    Effect::new(move |_| {
        if !open.get() {
            search.set(String::new());
        }
    });

    let trigger_label = move || -> String {
        let placeholder = t_string!(i18n, teams.kanban.selector_placeholder).to_string();
        current_team_label(
            &state.teams.get(),
            state.selected_team_id.get().as_deref(),
            &placeholder,
        )
    };

    let select_team = move |id: String| {
        state.selected_team_id.set(Some(id));
        open.set(false);
    };

    view! {
        <div class="relative">
            <button
                on:click=move |_| open.update(|v| *v = !*v)
                class="w-full flex items-center gap-1 px-2 py-1 rounded-md text-xs font-mono \
                       text-text-secondary border border-border \
                       bg-surface-raised hover:bg-surface-sunken hover:text-text-primary transition-colors"
                title=move || t_string!(i18n, teams.kanban.selector_label).to_string()
            >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                    <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" />
                    <circle cx="9" cy="7" r="4" />
                    <path d="M23 21v-2a4 4 0 0 0-3-3.87" />
                    <path d="M16 3.13a4 4 0 0 1 0 7.75" />
                </svg>
                <span class="flex-1 text-left truncate">{trigger_label}</span>
                <svg width="10" height="10" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="6 9 12 15 18 9" />
                </svg>
            </button>

            <Show when=move || open.get()>
                <div class="absolute top-full mt-2 left-0 z-50 w-full max-h-80 overflow-y-auto \
                            glass rounded-xl border border-border bg-surface-overlay/85 shadow-xl \
                            p-2 space-y-1"
                    on:mouseleave=move |_| open.set(false)>

                    {move || (!state.teams.get().is_empty()).then(|| view! {
                        <input
                            type="text"
                            placeholder=move || {
                                t_string!(i18n, teams.kanban.filter_placeholder).to_string()
                            }
                            class="w-full px-2.5 py-1.5 mb-1 rounded-md text-xs bg-surface-sunken \
                                   text-text-primary placeholder:text-text-tertiary outline-none \
                                   border border-border focus:border-primary/40"
                            on:input=move |ev| search.set(event_target_value(&ev))
                            on:keydown=move |ev| {
                                if ev.key() == "Escape" {
                                    open.set(false);
                                }
                            }
                            prop:value=move || search.get()
                        />
                    })}

                    {move || {
                        let teams = state.teams.get();
                        if teams.is_empty() {
                            return view! {
                                <div class="px-2.5 py-3 text-xs text-text-tertiary text-center">
                                    {t!(i18n, teams.kanban.empty_teams)}
                                </div>
                            }.into_any();
                        }
                        if filter_teams(&teams, &search.get()).is_empty() {
                            return view! {
                                <div class="px-2.5 py-3 text-xs text-text-tertiary text-center">
                                    {t!(i18n, teams.kanban.no_match)}
                                </div>
                            }.into_any();
                        }
                        view! {
                            <For
                                each=move || filter_teams(&state.teams.get(), &search.get())
                                key=|t: &TeamSummary| t.id.clone()
                                children=move |team: TeamSummary| {
                                    let id = team.id.clone();
                                    let id_active = id.clone();
                                    let name = team.name.clone();
                                    let is_active_team = team.status == "active";
                                    let is_selected = move || {
                                        state.selected_team_id.get().as_deref()
                                            == Some(id_active.as_str())
                                    };
                                    view! {
                                        <button
                                            on:mousedown=move |ev| ev.prevent_default()
                                            on:click=move |_| select_team(id.clone())
                                            class=move || {
                                                let base = "w-full text-left px-2.5 py-1.5 rounded-md \
                                                            text-xs transition-colors flex items-center \
                                                            gap-2 border";
                                                if is_selected() {
                                                    format!("{base} bg-primary/10 text-primary border-primary/30")
                                                } else {
                                                    format!("{base} hover:bg-surface-sunken text-text-secondary border-transparent")
                                                }
                                            }
                                        >
                                            <span class=move || {
                                                if is_active_team {
                                                    "w-2 h-2 rounded-full bg-success shrink-0"
                                                } else {
                                                    "w-2 h-2 rounded-full bg-text-tertiary shrink-0"
                                                }
                                            } />
                                            <span class="flex-1 truncate">{name}</span>
                                        </button>
                                    }
                                }
                            />
                        }.into_any()
                    }}
                </div>
            </Show>
        </div>
    }
}

fn event_target_value(ev: &leptos::ev::Event) -> String {
    use wasm_bindgen::JsCast;
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|s| s.value())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::teams::TeamSummary;

    fn team(id: &str, name: &str, status: &str) -> TeamSummary {
        TeamSummary {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            leader_id: "main".into(),
            status: status.into(),
            member_count: 0,
            task_count: 0,
            created_at: 0,
            disbanded_at: None,
            members_preview: Vec::new(),
            last_message: None,
            last_message_at: None,
        }
    }

    #[test]
    fn label_returns_selected_team_name() {
        let teams = vec![team("a", "Alpha", "active"), team("b", "Beta", "active")];
        assert_eq!(current_team_label(&teams, Some("b"), "Choose"), "Beta");
    }

    #[test]
    fn label_falls_back_to_placeholder_when_none_selected() {
        let teams = vec![team("a", "Alpha", "active")];
        assert_eq!(current_team_label(&teams, None, "Choose"), "Choose");
    }

    #[test]
    fn label_falls_back_to_placeholder_when_id_missing() {
        let teams = vec![team("a", "Alpha", "active")];
        assert_eq!(current_team_label(&teams, Some("gone"), "Choose"), "Choose");
    }

    #[test]
    fn filter_empty_query_returns_all() {
        let teams = vec![team("a", "Alpha", "active"), team("b", "Beta", "active")];
        assert_eq!(filter_teams(&teams, "   ").len(), 2);
    }

    #[test]
    fn filter_is_case_insensitive_substring() {
        let teams = vec![team("a", "Alpha", "active"), team("b", "Beta", "disbanded")];
        let got = filter_teams(&teams, "ET");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "b");
    }

    #[test]
    fn filter_no_match_returns_empty() {
        let teams = vec![team("a", "Alpha", "active")];
        assert!(filter_teams(&teams, "zzz").is_empty());
    }
}
```

- [ ] **Step 4: Run the helper tests on the host target**

Run: `cargo test -p aleph-panel --lib team_selector 2>&1 | tail -20`
Expected: PASS — 6 tests pass (`label_*` ×3, `filter_*` ×3).

> If `cargo test -p aleph-panel` fails to build for the host target due to a wasm-only dependency in the crate (unlikely — prior host-tested helpers exist in this crate, e.g. `chat_sidebar`'s `team_history_item_to_message`), fall back to verifying the helpers compile via the wasm check in Task 3 and note that the unit tests could not run on host. Do NOT add new dependencies to make host tests work.

> No commit here — fold into Task 4.

---

### Task 3: Relocate the pill to the top of `TeamsSidebar`

**Files:**
- Modify: `interfaces/webchat/src/views/teams/mod.rs` (`TeamsSidebar`, lines ~98-141)

**Interfaces:**
- Consumes: `components::team_selector::TeamSelector` (Task 2).
- Produces: updated `TeamsSidebar` layout (pill at top, sub-tabs below, no bottom selector block).

- [ ] **Step 1: Rewrite the `TeamsSidebar` view body**

In `interfaces/webchat/src/views/teams/mod.rs`, the current `TeamsSidebar` view is:

```rust
    view! {
        <div class="flex flex-col h-full">
            <div class="px-4 py-3">
                <h2 class="text-xs font-medium text-text-tertiary uppercase tracking-wider">
                    {move || t_string!(i18n, nav.teams).to_string()}
                </h2>
            </div>
            <nav class="flex-1 overflow-y-auto px-3 space-y-1">
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.overview).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Overview
                />
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.kanban).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Kanban
                />
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.plan).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Plan
                />
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.replay).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Replay
                />
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.workers).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Workers
                />
            </nav>
            <div class="px-3 py-3 border-t border-border">
                <components::team_selector::TeamSelector />
            </div>
        </div>
    }
```

Replace it with (pill moved to the top, "TEAMS" header dropped, bottom block removed):

```rust
    view! {
        <div class="flex flex-col h-full">
            <div class="px-3 py-3">
                <components::team_selector::TeamSelector />
            </div>
            <nav class="flex-1 overflow-y-auto px-3 space-y-1">
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.overview).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Overview
                />
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.kanban).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Kanban
                />
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.plan).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Plan
                />
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.replay).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Replay
                />
                <SubTabButton
                    label=Signal::derive(move || t_string!(i18n, teams.subtab.workers).to_string())
                    current=tab_state.sub_tab
                    target=TeamsSubTab::Workers
                />
            </nav>
        </div>
    }
```

Note: `i18n` is still used by the `SubTabButton` labels, so its binding and the `t_string` import remain needed — no import cleanup required. The `nav.teams` key is no longer referenced here but is still used elsewhere (e.g. the global nav), so leave that locale key in place.

- [ ] **Step 2: Compile-check the wasm target (covers Tasks 2 + 3)**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | tail -20`
Expected: clean (no errors). Warnings unrelated to this change may be ignored.

> No commit here — fold into Task 4.

---

### Task 4: Commit (GATED on user approval)

**Files:** all of the above.

- [ ] **Step 1: Ask the user before committing**

Per project rules, do NOT commit without explicit approval. Ask: "改动完成并通过 host 测试 + wasm 编译，是否提交？" Only proceed if the user says yes.

- [ ] **Step 2: Commit (only after approval)**

```bash
git add interfaces/webchat/src/views/teams/components/team_selector.rs \
        interfaces/webchat/src/views/teams/mod.rs \
        interfaces/webchat/locales/en.json \
        interfaces/webchat/locales/zh.json
git commit -m "webchat: move team selector to sidebar top as ModelPicker-style pill"
```

- [ ] **Step 3: Live E2E (user-driven)**

The user verifies in the running Panel: open the teams tab → pill sits at the top → clicking opens the popover with search + active/disbanded dots → selecting a team makes kanban/plan/replay follow → Overview still lists all teams. (Deploy/refresh per `DESKTOP_SHELL.md` rust_embed chain if testing in the .app.)

---

## Self-Review

**Spec coverage:**
- "Move selector to top" → Task 3. ✓
- "ModelPicker pill + popover style" → Task 2 (trigger classes, popover container, active highlight, filter, mouseleave/Escape, mousedown prevent_default). ✓
- "Verify selection drives sub-views" → wiring unchanged (Task 2 writes `selected_team_id`); documented in plan + Task 4 E2E. ✓
- "Overview keeps global list" → no Overview change made (explicitly a non-goal). ✓
- "Status dot active/disbanded" → Task 2 `bg-success` / `bg-text-tertiary` driven by `status == "active"`. ✓
- "Pure helper host-tested" → Task 2 `current_team_label` + `filter_teams` + 6 tests. ✓
- "Drop TEAMS header" → Task 3. ✓
- i18n keys → Task 1. ✓

**Placeholder scan:** No TBD/TODO; all code blocks are complete and copy-paste ready. ✓

**Type consistency:** `current_team_label(&[TeamSummary], Option<&str>, &str) -> String` and `filter_teams(&[TeamSummary], &str) -> Vec<TeamSummary>` are referenced identically in the component, tests, and Interfaces blocks. `TeamSummary` fields used in the test constructor match the struct (`id, name, description, leader_id, status, member_count, task_count, created_at, disbanded_at, members_preview, last_message, last_message_at`). Component name `TeamSelector` unchanged, so `mod.rs` import path is stable. ✓
