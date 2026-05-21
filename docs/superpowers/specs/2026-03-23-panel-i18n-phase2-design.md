# Panel UI i18n Phase 2 Design

## Summary

Translate all remaining ~230+ hardcoded English strings in the webchat panel to complete full Chinese language support. Phase 1 established the `leptos_i18n` infrastructure and translated navigation, sidebars, and settings page titles/descriptions. Phase 2 covers everything else: dashboard, chat, agents, memory, logs, cron, agent trace, connection status, and settings form fields.

## Context

- **Phase 1 completed:** Infrastructure (build.rs, locale files, I18nContextProvider), bottom bar, settings sidebar, all settings page titles/descriptions/loading/saving strings
- **Remaining:** ~230+ strings across 40+ files
- **Pattern established:** `use crate::i18n::*;` + `t!(i18n, key)` for views, `t_string!(i18n, key).to_string()` for String props, `Signal::derive(move || t_string!(i18n, key).to_string())` for Signal<String> props

## Approach

Same as Phase 1 — no architectural changes. Extend `en.json`/`zh.json` with new key groups, apply `t!()` in each file.

## Scope by Group

### Group 1: Dashboard (home.rs + dashboard_sidebar.rs) — ~35 strings

New locale keys under `dashboard.*`:
- `dashboard.title`, `dashboard.description`
- `dashboard.stats.*` (active_tasks, cpu_usage, memory_vault, gateway_latency)
- `dashboard.sections.*` (core_services, resources, recent_activity, quick_actions, system_info)
- `dashboard.system.*` (version, platform, uptime)
- `dashboard.resources.*` (cpu, memory, storage)
- `dashboard.actions.*` (restart, clear_buffer, export_memory)
- `dashboard.connection.*` (connect, disconnect, retry, connecting, required, required_desc, error)
- `dashboard.sidebar.*` (overview, agent_trace, memory_vault, scheduled_tasks, server_logs)
- `dashboard.activity.*` (event_log, view_all, no_activity, connect_to_view)

### Group 2: Chat (chat_sidebar.rs + chat/view.rs) — ~20 strings

New locale keys under `chat.*`:
- `chat.new`, `chat.search_placeholder`, `chat.no_conversations`, `chat.new_chat`
- `chat.confirm_delete`, `chat.back`
- `chat.thinking`, `chat.send_placeholder`, `chat.attach`, `chat.stop`, `chat.remove`

Special: `chat_sidebar.rs` has hardcoded Chinese strings (`"确认删除?"`, `"确认"`, `"取消"`) — replace with `t!(i18n, chat.confirm_delete)`, `t!(i18n, common.confirm)`, `t!(i18n, common.cancel)`.

### Group 3: Memory + Logs + Agent Trace — ~49 strings

New locale keys:
- `memory.*` (title, description, tabs, search, table headers, empty states)
- `logs.*` (title, description, filters, empty states)
- `trace.*` (title, description, controls, event messages, empty states)

### Group 4: Cron — ~45 strings

New locale keys under `cron.*`:
- `cron.title`, `cron.description`
- `cron.form.*` (name, schedule_type, schedule, agent, prompt, timezone, tags, status, etc.)
- `cron.types.*` (cron, every, at)
- `cron.status.*` (enabled, disabled)
- `cron.actions.*` (run_now, save, delete, cancel, confirm_delete)
- `cron.history.*` (title, status, time, duration, delivery, error, no_records)
- `cron.empty.*` (no_tasks, select_job, new_task, edit_task)

### Group 5: Agents pages (agents/*.rs + agents_sidebar.rs) — ~69 strings

New locale keys under `agents.*`:
- `agents.new_agent`, `agents.loading`
- `agents.form.*` (id, name, placeholders, validation)
- `agents.overview.*` (identity, emoji, description, theme, model config labels)
- `agents.behavior.*` (system_prompt, memory, reasoning)
- `agents.channels.*` (bindings, configuration)
- `agents.skills.*` (assigned, available)
- `agents.tools.*` (custom, built_in)

### Group 6: Connection status + misc components — ~6 strings

- `common.connected`, `common.disconnected` (already exist)
- `common.reconnecting`

### Group 7: Settings form fields — ~80 strings

Extend existing `settings.*` keys with form-level detail:
- `settings.behavior.output_mode`, `settings.behavior.typing_speed`, etc.
- `settings.security.require_auth`, `settings.security.device_pairing`, etc.
- Each settings page gets sub-keys for its form fields

## What is NOT translated

- `console.log` / `console.error` debug messages
- File size units (KB, MB) — internationally standard
- Brand names (Aleph, Telegram, Discord, WhatsApp, etc.)
- API error messages from backend (dynamic, not in locale files)
- Code/technical identifiers

## Testing

Same as Phase 1:
- Compile-time: `leptos_i18n` validates key completeness
- `cargo check -p aleph-panel --target wasm32-unknown-unknown`
- Manual browser verification: switch to Chinese, verify all pages
