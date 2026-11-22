# Classification & routing — IF1 (minimax-cn/MiniMax-M3.1-Flash)

| ID | Route | fix_shape | risk | note |
|---|---|---|---|---|
| IF1-01 | REJECT | no-code-change | none | refuted by adjudication |
| IF1-02 | FIX-NOW | local-edit | low | Reject duplicate account, group, and topic identifiers before building lookup. |
| IF1-03 | FIX-NOW | multi-site | medium | Honor effective block flag while preserving topic-over-group precedence. |
| IF1-04 | FIX-NOW | local-edit | medium | Fail closed on proxy errors instead of silently connecting directly. |
| IF1-05 | FIX-NOW | local-edit | medium | Keep approval state committed while surfacing delivery failures. |
| IF1-06 | FIX-NOW | local-edit | medium | Return attachment failure unless one attachment truly succeeds. |
| IF1-07 | FIX-NOW | multi-site | medium | Deny AdminOnly delivery without verified administrator authorization. |
| IF1-08 | REJECT | no-code-change | none | refuted by adjudication |
| IF1-09 | FIX-NOW | multi-site | medium | Apply account-aware access checks before enqueueing poll or reaction events. |
| IF1-10 | FIX-NOW | multi-site | medium | Cancel health tasks on stop, startup failure, and channel shutdown. |
| IF1-11 | FIX-NOW | local-edit | medium | Release tracker lock before awaiting Telegram; commit state afterward. |
| IF1-12 | FIX-NOW | multi-site | medium | Apply Silent and AdminOnly policy before publishing streaming errors. |
| IF1-13 | FIX-NOW | multi-site | medium | Reuse effective per-chat config and the shared cooldown owner. |
| IF1-14 | FIX-NOW | multi-site | low | Pass actual inbound message ID through orchestrator context. |
| IF1-15 | FIX-NOW | multi-site | high | Ensure final text finalizes before media delivery begins. |
| IF1-16 | FIX-NOW | local-edit | medium | Resolve exact chat or topic mappings before account-level fallback. |
| IF1-17 | FIX-NOW | multi-site | medium | Bind approval delivery to the conversation's owning Telegram account. |
| IF1-18 | FIX-CAREFULLY | local-edit | low | Keep check-and-record atomic per scope without global locking. |
| IF1-19 | FIX-CAREFULLY | local-edit | low | Reset throttle state after failed first-preview delivery. |
| IF1-20 | DEFER | multi-site | high | Define durable-before-memory acknowledgement semantics before implementation. |
