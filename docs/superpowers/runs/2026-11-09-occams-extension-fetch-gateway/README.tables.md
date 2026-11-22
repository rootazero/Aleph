### Batch coverage

| Batch | Module | Files | LOC | Candidates (cheap) | Deep findings | Verdict (UPHELD/REF/DOWN) | Surviving P0/P1/P2/P3 | Fix commit |
|---|---|---:|---:|---:|---:|---|---|---|
| F1 | src/fetch — fetch provider contract (SSRF, redaction, size limits) | 5 | 380 | 19 | 5 | 0/2/3 | 0/0/0/0 | — |
| E1 | src/extension — lifecycle, loader, template, mod.rs (plugin lifecycle state machine) | 5 | 13,278 | 35 | 3 | 1/2/0 | 0/1/0/0 | — |
| E2 | src/extension — root services (plugin ops, visibility, state, watcher, service manager) | 27 | 11,135 | 40 | 2 | 1/0/1 | 0/1/0/0 | — |
| E3 | src/extension — hooks executor + hook types (ordering, consent, idempotency) | 20 | 13,201 | 48 | 3 | 2/0/1 | 1/1/0/0 | — |
| E4 | src/extension — manifest parsing/validation + effects (schema, fail-closed, effect claims) | 22 | 9,758 | 69 | 6 | 6/0/0 | 1/4/1/0 | — |
| E5 | src/extension — WASM runtime + registry/registrar + mcp_config (sandbox, single source of truth) | 20 | 8,888 | 41 | 6 | 5/0/1 | 2/3/0/0 | — |
| E6 | src/extension — marketplace install + panel publication (supply chain, publish path) | 12 | 3,543 | 38 | 5 | 4/0/1 | 4/0/0/0 | — |
| G1 | src/gateway — channel registry + resume coordinator + event visibility + delivery queue | 8 | 23,207 | 43 | 6 | 5/0/1 | 0/4/1/0 | — |
| G2 | src/gateway — session projector + visibility rules + i18n (single source of truth for state) | 9 | 11,879 | 29 | 3 | 3/0/0 | 1/0/2/0 | — |
| G3 | src/gateway — root part A (event bus, agent env, middleware glue, formatting) | 35 | 14,664 | 50 | 3 | 3/0/0 | 0/1/2/0 | — |
| G4 | src/gateway — root part B (misc root services) | 44 | 18,392 | 61 | 10 | 7/0/3 | 1/5/1/0 | — |
| H1 | src/gateway/handlers — part A (agent/users/projects/message CRUD handlers) | 21 | 14,665 | — | 8 | 7/0/1 | 1/4/2/0 | — |
| H2 | src/gateway/handlers — part B (session/config/provider handlers) | 22 | 14,698 | — | 9 | 9/0/0 | 3/4/2/0 | — |
| H3 | src/gateway/handlers — part C (tool/memory/misc handlers) | 22 | 14,696 | — | 7 | 5/0/2 | 1/4/0/0 | — |
| H4 | src/gateway/handlers — part D (auxiliary handlers) | 22 | 14,689 | — | 7 | 7/0/0 | 2/4/1/0 | — |
| H5 | src/gateway/handlers — teams + session db_handlers + memory + providers | 24 | 15,233 | — | 18 | 17/0/1 | 2/14/1/0 | — |
| H6 | src/gateway/handlers — graph, cron, plugins, generation_providers, search_config, extensions, security_config, config, auth | 34 | 9,489 | — | 10 | 6/0/4 | 2/2/2/0 | — |
| IF1 | src/gateway/interfaces — telegram (+streaming) channel adapter | 25 | 8,232 | — | 20 | 14/2/4 | 0/12/2/0 | — |
| IF2 | src/gateway/interfaces — discord + voice_call adapters (incl. relay tests) | 26 | 12,304 | — | 13 | 8/0/5 | 2/5/1/0 | — |
| IF3 | src/gateway/interfaces — feishu + whatsapp adapters (runtime/policy/auth splits) | 54 | 11,537 | — | 21 | 21/0/0 | 4/14/3/0 | — |
| IF4 | src/gateway/interfaces — imessage + matrix + line + xmpp adapters | 40 | 10,751 | — | 35 | 24/1/10 | 7/14/3/0 | — |
| IF5 | src/gateway/interfaces — slack + qq + wechat + nostr adapters | 42 | 9,991 | — | 22 | 20/0/2 | 4/12/4/0 | — |
| IF6 | src/gateway/interfaces — sms, irc, signal, email, mattermost, webhook + root | 23 | 12,225 | — | 16 | — | — | — |
| EE1 | src/gateway/execution_engine — part A (execute core, tool dispatch) | 19 | 12,940 | — | 4 | 4/0/0 | 1/2/1/0 | — |
| EE2 | src/gateway/execution_engine — part B + run_loop (streaming, cancellation) | 26 | 21,248 | — | 2 | 2/0/0 | 0/1/1/0 | — |
| EE3 | src/gateway/execution_engine — tests + slash command body + skill pregrant + btw_promote | 14 | 6,432 | — | 4 | 3/0/1 | 1/0/1/1 | — |
| GX1 | src/gateway — server + control_plane + admin_api (IPC surface, R4/R6 boundary) | 30 | 16,873 | — | 11 | 10/0/1 | 4/2/4/0 | — |
| GX2 | src/gateway — telecom + inbound_router (routing, R5 delivery, R4 pure I/O) | 27 | 20,563 | — | — | — | — | — |
| GX3 | src/gateway — session_store + session_manager + busy_queue (state ownership, backpressure) | 24 | 17,556 | — | 7 | — | — | — |
| GX4 | src/gateway — pty + event_emitter + events (process IO, event fanout) | 31 | 13,248 | — | 12 | 11/0/1 | 1/4/6/0 | — |
| GX5 | src/gateway — security + middleware + surface + link + message_assembly + pipeline | 39 | 9,593 | — | 3 | 0/2/1 | 0/0/0/0 | — |
| GX6 | src/gateway — voice + openai_api + btw + mcp_face + agent_env + formatter | 50 | 18,260 | — | 17 | — | — | — |
| **TOTAL** | **32 batches** | **822** | **413,548** | **473** | **298** | **205/9/44** | **45/118/41/1** | 0 commits |

### Fix commits (in order)

_no commits on the fix branch yet_

### Per-batch fix notes

| Batch | Findings fixed | Note |
|---|---:|---|
| F1 | 5 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| E1 | 3 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| E2 | 2 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| E3 | 3 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| E4 | 6 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| E5 | 6 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| E6 | 5 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| G1 | 6 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| G2 | 3 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| G3 | 3 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| G4 | 10 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| H1 | 8 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| H2 | 9 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| H3 | 7 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| H4 | 7 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| H5 | 18 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| H6 | 10 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| IF1 | 20 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| IF2 | 13 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| IF3 | 21 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| IF4 | 35 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| IF5 | 22 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| IF6 | 0 | — |
| EE1 | 4 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| EE2 | 2 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| EE3 | 4 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| GX1 | 11 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| GX2 | 0 | — |
| GX3 | 0 | — |
| GX4 | 12 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| GX5 | 3 | - worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx |
| GX6 | 0 | — |
