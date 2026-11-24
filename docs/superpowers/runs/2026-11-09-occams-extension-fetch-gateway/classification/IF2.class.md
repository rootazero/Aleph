# Classification & routing — IF2 (minimax-cn/MiniMax-M3.1-Flash)

| ID | Route | fix_shape | risk | note |
|---|---|---|---|---|
| IF2-01 | FIX-NOW | local-edit | low | Preserve serde format while redacting only Debug token output. |
| IF2-02 | FIX-NOW | multi-site | medium | Preserve default discord identity while threading configured IDs end-to-end. |
| IF2-03 | FIX-NOW | local-edit | medium | Preserve marker semantics and reconstruct every original character in order. |
| IF2-04 | FIX-NOW | local-edit | medium | ACK exactly once before execution; still report failures without re-execution. |
| IF2-05 | FIX-NOW | multi-site | low | Reject zero IDs as ChannelError without changing valid snowflake formats. |
| IF2-06 | FIX-NOW | local-edit | medium | Distinguish startup failure from post-READY reconnect behavior. |
| IF2-07 | FIX-NOW | local-edit | low | Add GUILDS only when thread events are enabled. |
| IF2-08 | FIX-NOW | local-edit | low | Match all guild candidates before applying existing ambiguity rules. |
| IF2-09 | FIX-NOW | local-edit | medium | Do not cache snapshots after unknown fetch failures. |
| IF2-10 | FIX-NOW | local-edit | low | Apply guild allowlist only when a guild channel is present. |
| IF2-11 | DEFER | multi-site | high | Preserve revoke, persist, permit-release ordering during blocking finalization. |
| IF2-12 | FIX-CAREFULLY | multi-site | medium | Await prior connection exit before allowing a new start. |
| IF2-13 | FIX-CAREFULLY | local-edit | medium | Return complete guild snapshots only after all pages succeed. |
