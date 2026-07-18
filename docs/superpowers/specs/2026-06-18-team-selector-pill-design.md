# Team Selector Pill — Design

**Date:** 2026-06-18
**Status:** Approved (design), pending implementation plan
**Area:** `interfaces/webchat` (Leptos/WASM Panel), Teams tab

## Problem

In the Panel Teams tab, the "current team" selector is a plain native `<select>`
pinned to the **bottom** of the teams sidebar (`TeamsSidebar`). It is easy to
miss, and its visual style does not match the polished `ModelPicker` selector in
the chat composer. When a user switches between the chat tab and the teams tab,
the selection experience feels unfamiliar.

Goals:

1. Move the team selector to the **top** of the teams sidebar so it is the first,
   most prominent control.
2. Restyle it to match the chat window's `ModelPicker` (pill trigger + popover),
   so switching tabs feels consistent and reduces operational unfamiliarity.
3. Verify selecting a team drives the sub-views (kanban / plan / replay) below.

## Functional finding (no data bug)

Investigation confirmed the wiring already works: `kanban.rs`, `plan_dag.rs`, and
`replay.rs` each hold a reactive `Effect` that reads
`TeamsTabState.selected_team_id` and re-fetches via `TeamsApi::list_tasks(...)`
when it changes. The `Overview` sub-view intentionally lists **all** teams and
does not follow the selection (it is the global overview). So "select a team →
kanban/plan/replay show that team" is already true. This change is a UI
relocation + restyle, **not** a data-sync fix. The reactive wiring is preserved
unchanged.

## Reference style (decided)

Mirror the chat composer's **`ModelPicker`** (`components/model_picker.rs`):
pill trigger (icon + current label + chevron) that opens a floating popover with
an active-item highlight. Chosen over the native `<select>` and over an
agent-list-style nav-tile list.

## Overview behavior (decided)

`Overview` **keeps the global all-teams list** (cards, create/disband management).
The selector drives only kanban / plan / replay. Rationale: clear semantics —
Overview = global summary; other sub-tabs = single-team workspace. Minimal change,
preserves the existing create/disband entry point.

## Design

### Component: `TeamSelector` (rewrite, same name)

File: `interfaces/webchat/src/views/teams/components/team_selector.rs`.
Keep the component name `TeamSelector` so `mod.rs`'s import path is unchanged.
Rewrite its body to mirror `ModelPicker`:

- **Trigger pill**: a team-glyph SVG + current team name + chevron. Same Tailwind
  classes as `ModelPicker`'s trigger
  (`flex items-center gap-1 px-2 py-1 rounded-md text-xs ... bg-surface-raised
  hover:bg-surface-sunken hover:text-text-primary transition-colors`).
  The label text is computed by a pure helper `current_team_label(teams,
  selected_id) -> String` (extracted for host unit testing).
- **Popover** (`Show when=open`): `absolute` positioned. **Opens downward**
  (`top-full mt-2`) because the pill sits at the top of the sidebar — this is the
  one intentional divergence from `ModelPicker`, which opens upward
  (`bottom-full mb-2`) since it lives above the composer. Reuse the popover
  container classes (`glass rounded-xl border border-border
  bg-surface-overlay/85 shadow-xl p-2 space-y-1`) with an appropriate width.
- **Rows**: each team = status dot + name. Active (selected) row gets
  `bg-primary/10 text-primary border border-primary/30`; others
  `hover:bg-surface-sunken text-text-secondary border border-transparent`.
- **Status dot**: green for active teams, grey for disbanded teams. Color is
  driven by `TeamSummary` status (exact field name verified at implementation).
  Disbanded teams remain listed and selectable so their historical kanban/plan
  can still be inspected.
- **Filter input**: included for parity with `ModelPicker` (order-preserving
  substring match). Harmless when few teams exist.
- **Interactions**: click toggles open; `on:mouseleave` closes; `Escape` closes.
  Armed/interactive buttons use `on:mousedown` `prevent_default()` to avoid the
  known macOS WebKit focus/blur bug.
- **On select**: `state.selected_team_id.set(Some(id))` then close the popover.

### Layout: `TeamsSidebar` (`views/teams/mod.rs`)

- Place `<TeamSelector />` at the **top** of the sidebar (replacing the existing
  "TEAMS" text header block — the pill itself conveys context, matching
  `ModelPicker`'s label-less look).
- Keep the 5 sub-tab nav buttons below, unchanged.
- **Remove** the bottom `border-t` block that previously hosted the selector.

### Data flow (unchanged)

Pill writes `selected_team_id` → existing kanban/plan/replay Effects react and
re-fetch. Overview continues reading the full `state.teams` list. No backend or
RPC changes.

## Edge cases

- **No teams**: pill shows a placeholder ("Select team"); popover shows an empty
  state message.
- **Disbanded teams**: listed with a grey dot; still selectable.
- **Reconnect**: the existing load Effect in `TeamsView` already preserves the
  current selection (or falls back to the first team); unchanged.

## Files touched

- `interfaces/webchat/src/views/teams/components/team_selector.rs` — rewrite
  `TeamSelector` to pill + popover; add pure `current_team_label` helper + its
  unit test.
- `interfaces/webchat/src/views/teams/mod.rs` — move the pill to the top, drop
  the "TEAMS" header text, remove the bottom selector block.
- i18n — reuse existing `teams.kanban.selector_label` / `selector_placeholder`;
  add (or reuse) a key for the popover filter placeholder.

## Testing

- Extract `current_team_label` as a pure function and cover it with a host unit
  test (label for a selected team, placeholder when none / id missing) — follows
  the project pattern of extracting host-testable helpers from view code.
- Compile-verify the WASM target: `cargo check -p aleph-panel --target
  wasm32-unknown-unknown`.
- Interactive E2E (open popover, select a team, confirm kanban/plan/replay
  follow, confirm Overview stays global) performed in the running Panel by the
  user. Observe extreme cargo restraint per project rules.

## Non-goals

- No change to Overview's all-teams listing or create/disband flow.
- No backend / RPC / data-model changes.
- No change to kanban/plan/replay fetch logic (only the selection entry point
  moves).
