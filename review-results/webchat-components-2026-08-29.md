# Module: `interfaces/webchat/src/components` (review 2026-08-29)

## Summary

- **Files**: 74 `.rs` files under `interfaces/webchat/src/components/`
- **LOC**: ~18 019 (excluding tests)
- **Top-level `.rs` files in scope**: none — every top-level `.rs` under `interfaces/webchat/src/` was explicitly excluded (`lib.rs`, `app.rs`, `context.rs`, `generation.rs`, `models.rs`, `preset_providers.rs`, `disposed_reads.rs`, `appearance.rs`, `i18n.rs`, `i18n_census.rs`, `platform_host.rs`, `panic_overlay.rs`); `api.rs` is the module declaration for the forbidden `api/` directory.
- **Issues**: 15 total (0 critical / 4 high / 10 medium / 1 low)
- **R2 Complex business UI in Leptos/WASM**: PASS — components are presentation-only and read from context/props; no business logic leaks into native shells.
- **R6 AI comes to you**: PASS — inline approval/ask-user cards, notification center, and artifact auto-open are wired; one race found in approval-card retry handling.
- **Wiring completeness**: PARTIAL — several presentation components call `expect_context` instead of `use_context`, and two modals `unwrap()` optional store values under a `Show` guard.

### What changed since the last components review
No prior components-only review existed; the last related audit was `utils-2026-08-29.md`. This is the first focused pass over the shared component layer. The largest new surface is the project-room suite (`project_page/*`, `sidebar/projects.rs`, `workspace_panel.rs`, `team_chat_entry.rs`, `team_participants.rs`, `team_task_strip.rs`) plus the extension trust/install flow (`extensions/*`).

## High-Confidence Issues

### [High] TrustModal unwraps optional disclosure under a `Show` guard
- **Location**: `interfaces/webchat/src/components/extensions/trust_modal.rs:25`
- **Description**: The modal reads `store.disclosure.get().unwrap()` inside the child closure of `<Show when=move || store.disclosure.get().is_some()>`. The guard and the inner read are two separate reactive evaluations; if `disclosure` flips to `None` between them (user cancels, a new install starts, or the store resets), the view panics.
- **Risk / trigger**: A panic inside a security-critical trust gate aborts the install flow and can leave the UI in a broken state. Race is reachable by clicking the scrim or pressing Escape while a disclosure event is in flight.
- **Fix**: Read the option once inside the child closure with `if let Some(d) = store.disclosure.get() { ... }` (or map the `Option` to a view). Make the context read `use_context` with an early return instead of `expect_context`.
- **Decision**: FIXED in this round.
- **Suggested test**: A red-team test that toggles `disclosure` between `Some` and `None` rapidly must not panic.

### [High] ExtensionDetailDrawer unwraps `store.selected` under a `Show` guard
- **Location**: `interfaces/webchat/src/components/extensions/detail_drawer.rs:49`
- **Description**: Same anti-pattern: `store.selected.get().unwrap()` inside the child of a `Show when=store.selected.get().is_some()`.
- **Risk / trigger**: Rapid open/close of the drawer, or a connection reset that clears `selected`, can panic the extensions store surface.
- **Fix**: Use `if let Some(entry) = store.selected.get()` inside the closure and make the context read `use_context` with an early return.
- **Decision**: FIXED in this round.
- **Suggested test**: Mount the drawer, set `selected = Some(...)`, then set `selected = None` during render; assert no panic.

### [High] JsonSchemaForm renders untrusted `how_to_get_url` links raw
- **Location**: `interfaces/webchat/src/components/json_schema_form.rs:193-200`
- **Description**: The install-config wizard renders the catalog-supplied `how_to_get_url` as `<a href=url target="_blank" rel="noopener noreferrer">` without validating the URL scheme.
- **Risk / trigger**: A compromised catalog entry or a MITM response can inject `javascript:` / `data:` links into the install flow. The browser follows them from the panel origin.
- **Fix**: Run every `how_to_get_url` through the existing `crate::components::markdown::sanitize_link_url` helper before rendering; reject disallowed schemes by rendering plain text instead of an anchor.
- **Decision**: FIXED in this round.
- **Suggested test**: Render a field whose `how_to_get_url` is `javascript:alert(1)`; assert the output contains `#disallowed-` and no `href="javascript:`.

### [High] ExtensionDetailDrawer renders `repo_url` raw
- **Location**: `interfaces/webchat/src/components/extensions/detail_drawer.rs:86`
- **Description**: `entry.repo_url` is emitted as a plain `<a href=url target="_blank" ...>` without scheme validation.
- **Risk / trigger**: Malicious extension metadata can pivot XSS through a `javascript:` repo URL shown on the detail card.
- **Fix**: Sanitize `repo_url` with the same `sanitize_link_url` helper; fall back to a non-clickable label when the scheme is disallowed.
- **Decision**: FIXED in this round.
- **Suggested test**: Same red-team shape as H3, on the detail drawer.

### [Medium] ExtensionDetailDrawer discards disclosure fetch errors
- **Location**: `interfaces/webchat/src/components/extensions/detail_drawer.rs:29-39`
- **Description**: The `ExtensionsApi::disclosure` `Err(_)` arm only sets `disc_loading.set(false)`, leaving `disclosure` as `None`. The user sees the localized "no permissions" copy regardless of whether the call failed due to a network/admin/transport error.
- **Risk / trigger**: A refused permission or a transient network error is misreported as "this extension needs no permissions", masking real failures.
- **Fix**: Add a `disc_error` signal; route the error through `admin_refusal::settings_load_error` and render it above the permission body.
- **Decision**: FIXED in this round.
- **Suggested test**: Return `Err(ADMIN_REQUIRED_MESSAGE)` from the disclosure API; assert the rendered text is the localized admin refusal, not `extensions.no_perms`.

### [Medium] ApprovalCard resolve errors only log to console
- **Location**: `interfaces/webchat/src/components/approval_card.rs:31-37`
- **Description**: On `ExecApprovalApi::resolve` error the component calls `web_sys::console::warn_1` but updates no store/signal. The approval remains visible and clickable.
- **Risk / trigger**: A failed resolve (network blip, already-expired approval, admin refusal) leaves the card on screen; the operator is likely to click again, producing duplicate or conflicting resolutions.
- **Fix**: Add a local `resolve_error` signal and render it inside the card; disable/hide action buttons after a non-retryable error.
- **Decision**: FIXED in this round.
- **Suggested test**: Mock the API to return `Err("refused")`; assert the error text appears and the allow/deny buttons are disabled.

### [Medium] ApprovalCard buttons are not disabled during a pending resolve
- **Location**: `interfaces/webchat/src/components/approval_card.rs:139-180`
- **Description**: Each button spawns its own `spawn_local` resolve task without a shared "resolving" guard. Clicks while a resolve is in flight send additional requests.
- **Risk / trigger**: Rapid double-clicks or impatient retries can race on the server.
- **Fix**: Add a local `resolving: RwSignal<bool>` that is set when any resolve starts and unset on completion; wire all four action buttons (allow-once, allow-session, allow-always, deny) and the deny-with-reason submit to `disabled=move || resolving.get()`.
- **Decision**: FIXED in this round.
- **Suggested test**: Click "Allow once" twice in quick succession; assert only one RPC is issued.

### [Medium] AskUserCard answer errors only log to console and allows duplicate submission
- **Location**: `interfaces/webchat/src/components/ask_user_card.rs:53-67`
- **Description**: `ClarificationApi::resolve*` errors are written to `console::warn_1`; the card stays open and the submit button is not locked. The same answer can be sent repeatedly.
- **Risk / trigger**: Same pattern as the approval card: a failed answer leaves the UI in a state that invites retries, which may re-execute a clarification action the server already processed.
- **Fix**: Add an `answer_error` signal rendered under the question, and a local `submitting` signal that disables the submit/option buttons while a request is in flight.
- **Decision**: FIXED in this round.
- **Suggested test**: Submit an answer while the API returns an error; assert the error is shown and the button is disabled during the call.

### [Medium] TokenWall uses `expect_context` for `DashboardState`
- **Location**: `interfaces/webchat/src/components/token_wall.rs:16`
- **Description**: The full-screen credential gate calls `expect_context::<DashboardState>()` instead of `use_context`.
- **Risk / trigger**: Mounting the component in storybook, a test harness, or a route that omits the provider panics.
- **Fix**: Replace with `use_context` and return an empty view when the context is missing.
- **Decision**: FIXED in this round.

### [Medium] Project-room Kanban tab silently discards task-list fetch errors
- **Location**: `interfaces/webchat/src/components/project_page/kanban.rs:207-214`
- **Description**: `RoomTeamCard` uses `if let Ok(list) = TeamsApi::list_tasks(...)` and drops the `Err`. A failed task load looks like zero tasks.
- **Risk / trigger**: A network/admin error hides failed coordination state from room members.
- **Fix**: Add a per-team `task_error` signal; render it under the team card and route admin refusals through `admin_refusal::settings_load_error`.
- **Decision**: FIXED in this round.

### [Medium] Widespread `expect_context` usage in presentation components
- **Locations** (representative; grep found ~24 occurrences):
  - `interfaces/webchat/src/components/project_page/kanban.rs:95` (`DashboardState`, `ChatState`)
  - `interfaces/webchat/src/components/project_page/memory.rs:55` (`DashboardState`)
  - `interfaces/webchat/src/components/project_page/workspace.rs:49` (`DashboardState`)
  - `interfaces/webchat/src/components/project_page/settings.rs:38-42` (`DashboardState`, `UserDirectoryState`, `ProjectsTabState`)
  - `interfaces/webchat/src/components/project_page.rs:121` (`ProjectsTabState`); `project_page.rs:150` (`DashboardState`, `UserDirectoryState`)
  - `interfaces/webchat/src/components/model_picker.rs:95-96` (`DashboardState`, `ChatState`)
  - `interfaces/webchat/src/components/tool_card.rs:452` (`ChatState`)
  - `interfaces/webchat/src/components/workspace_panel.rs:22` (`WorkspaceState`, `ChatState`)
  - `interfaces/webchat/src/components/sidebar/session_status_bar.rs:24-25` (`DashboardState`, `SessionMap`)
  - `interfaces/webchat/src/components/sidebar/sidebar_item.rs:19` (`DashboardState`)
  - `interfaces/webchat/src/components/agents_sidebar.rs:16` (`DashboardState`)
  - `interfaces/webchat/src/components/ui/agent_binding_selector.rs:20` (`DashboardState`)
- **Description**: Presentation components that merely render data declare required context with `expect_context`, which panics when the context is absent. This is acceptable in the main app (where providers are always present) but violates the "storybook/standalone/test" robustness expected of shared components and makes every one a latent panic surface.
- **Risk / trigger**: Future routes, tests, storybook mounts, or provider refactors can crash the panel at these sites.
- **Fix**: Convert to `use_context` with an early-return fallback. The three project-page tabs, `model_picker`, `tool_card`, and `sidebar` items are the highest-value conversions because they are shared across routes.
- **Decision**: DEFERRED (rationale: the change is mechanical and correct, but it touches ~24 call sites across the entire layer. Doing it in this round would drown the higher-impact XSS/panic fixes; it should be a standalone wiring-hardening change with a source-level guard that forbids new `expect_context` in `components/`).
- **Suggested test**: A compile-time lint/guard that greps for `expect_context` under `interfaces/webchat/src/components/` and fails the build on new occurrences; plus a storybook smoke test that mounts each component without context and asserts no panic.

### [Medium] JsonSchemaForm default-seeding effect is not reactive to `fields` changes
- **Location**: `interfaces/webchat/src/components/json_schema_form.rs:155-164`
- **Description**: The effect that seeds default values captures a clone of the initial `fields` prop and reads no reactive dependency. If the install wizard advances to a new extension with a different schema, defaults for the new required fields are never inserted.
- **Risk / trigger**: Multi-step install flows, or reopening the configure step for a different extension, can submit empty required fields and fail server-side validation.
- **Fix**: Make the effect depend on a reactive view of `fields` (e.g., accept `#[prop(into)] fields: Signal<Vec<FieldSpec>>` or keep a `RwSignal` of fields inside the component and seed whenever it changes). The seeding logic already uses `or_insert_with`, so it will not overwrite user edits.
- **Decision**: DEFERRED (rationale: requires an API change to make the prop reactive; should be validated against real multi-step install flows to avoid re-seeding loops).

### [Medium] WorkspacePanel team tabs silently discard fetch errors
- **Locations**: `interfaces/webchat/src/components/workspace_panel.rs:108-114` (`TeamDeliverablesView`), `interfaces/webchat/src/components/workspace_panel.rs:170-180`, `188-194` (`TeamTasksView`)
- **Description**: Both team-mode tabs use `if let Ok(...) = ...` and ignore errors. A failed `team_chat.thread` or `teams.get` leaves the tab empty with no feedback.
- **Risk / trigger**: Network/admin failures look like "no deliverables/tasks".
- **Fix**: Add `error` signals to both tabs and render through `admin_refusal::settings_load_error`.
- **Decision**: DEFERRED (rationale: low user-facing impact for MVP; the empty state is not catastrophically wrong. Should be bundled with the team-mode error-handling pass.)

### [Low] SettingsTab roster list is not keyed
- **Location**: `interfaces/webchat/src/components/project_page/settings.rs:170-220`
- **Description**: `project.member_ids.into_iter().map(...)` renders the roster without `<For key=...>`. The member list is static per `ProjectInfo` prop, but prop changes can cause reconciliation churn.
- **Risk / trigger**: Adding/removing members may reset focus/selection state of sibling rows in edge cases.
- **Fix**: Replace the `map` with `<For each=move || project.member_ids.clone() key=|uid| uid.clone() ...>`.
- **Decision**: DEFERRED (rationale: cosmetic / reconciliation hygiene; no observed bug today.)

## Per-perspective findings

### Security
- **Markdown XSS surface**: `markdown.rs` correctly escapes raw HTML/inline HTML events, escapes fence info-strings, and sanitizes link/image URL schemes (`javascript:`, `data:`, protocol-relative `//`). No raw user content reaches `inner_html` without escaping. PASS.
- **Artifact/preview links**: `artifacts/row.rs`, `artifacts/deliverable.rs`, and `artifacts/preview.rs` render artifact URLs as plain `<a href=... target="_blank">` without scheme validation. These URLs are server-issued capability paths, so the trust boundary is the server/catalog, not another user. Acceptable, but consistent sanitization would be defense-in-depth.
- **Trust/install link injection**: H3 and H4 above are the only user-controlled URL surfaces that were not sanitized.
- **TokenWall**: Password input is correctly typed `password` and the submit button is disabled while the input is blank. No XSS or injection surface.

### Logic
- **Approval policy**: `approval_card.rs` correctly derives the offered decisions from `approval.allowed_decisions` (server-supplied), and the deny-with-reason flow correctly requires a non-empty objection. The new `resolving` guard (this round) removes the duplicate-approval race.
- **Kanban room membership**: `project_page/kanban.rs` uses `aleph_protocol::scope::belongs_to_project` for client-side filtering and has unit tests for the three ways a team can fail to belong. Empty board states are handled for teams, goals, and loops.
- **Workspace three-valued state**: `project_page/workspace.rs` distinguishes `None` (loading), `Some(false)` (unbound), and `Some(true)` (bound) and does not conflate them.
- **Memory tab scope**: `project_page/memory.rs` composes the partition id via `room_partition(agent_id, project_id)` and pins the separator in tests.
- **Typewriter/markdown streaming**: `markdown.rs` correctly handles stale cached offsets, char-boundary safety for CJK, and incremental rendering. The only weakness is the absence of an upper bound on the streaming render buffer (M8). A malicious or runaway model could produce a very large chunk; the current allocation is `content.len() * 2` with no cap.

### Architecture (R1–R10)
- **R2 / R6**: All components remain presentation-only; no business logic or native API calls. The approval/ask-user cards implement R6 inline interactions correctly.
- **R3**: No heavy dependencies introduced in this layer.
- **R4**: Interface layer stays pure I/O; state mutations are delegated to the `DashboardState` / `StoreState` contexts.
- **R8 / R10**: No regex-based intent parsing; tool-kind matching in `tool_card.rs` is string-name dispatch, not semantic classification, and is appropriate for rendering.
- **Wiring**: The main architectural gap is the `expect_context` cluster (M10). Several components do use `use_context` with graceful fallback (`team_task_strip.rs`, `connection_status.rs`, `notification_center.rs`, `boot_check_gate.rs`, `service_blocking_gate.rs`, `approval_card.rs`, `ask_user_card.rs`), so the correct pattern is already present in the codebase and should be extended.

### Quality
- **Host-testable logic**: `tool_card.rs`, `markdown.rs`, `project_page/kanban.rs`, `project_page/workspace.rs`, `project_page/memory.rs`, `artifacts/*`, `admin_refusal.rs`, `picker_nav.rs`, `team_chat_entry.rs`, `team_participants.rs`, and `extensions/install_flow.rs` all have `#[cfg(test)]` blocks covering their pure helpers. Good coverage.
- **Error classification**: `admin_refusal.rs` is a well-designed chokepoint; most RPC call sites route load/write errors through it. The remaining gaps are the silent `if let Ok(...)` sites in team-mode and `RoomTeamCard`.
- **Memory hygiene**: `team_participants.rs` creates a `web_sys::ResizeObserver` with a `Closure::new(...)` and then calls `cb.forget()`, which intentionally leaks the JS closure. The observer itself is disconnected on cleanup, but the closure is leaked on every effect re-run. This is a bounded leak only if the component re-runs rarely; a more robust shape would store the `Closure` in a `StoredValue` and drop it in `on_cleanup`. Not fixed in this round because the change is non-trivial and the current pattern prevents a use-after-free.

## Conclusion

The webchat component layer is largely well-architected and matches Aleph's redlines (R2, R6, R4, R8, R10). The highest-impact defects are concentrated in the new extension trust/install flow (two `unwrap()` panics and two unsanitized URL sinks) and the approval/ask-user inline cards (silent errors + duplicate-submission races). The four High findings and six cheap Medium findings were fixed in this round. The remaining Mediums are mechanical wiring hardening (`expect_context` sweep) and deferred error-surface work that should be tackled as a follow-up with a source-level guard.

### What was not done (skipped validations)
- No `cargo check` / `cargo test` / `clippy` was run per instructions; all fixes are syntactic and need a real build to verify.
- The `expect_context` sweep was not applied across all ~24 sites; only the security/privacy-critical surfaces (`TokenWall`, `TrustModal`, `ExtensionDetailDrawer`) were converted.
- The unbounded markdown streaming buffer cap was not added; it requires a product decision on max rendered size and a corresponding truncation UI.
- Team-mode fetch errors in `workspace_panel.rs` and the `JsonSchemaForm` reactivity issue were deferred.
- The `team_participants.rs` ResizeObserver closure leak was left as a known bounded leak with a documented rationale.
