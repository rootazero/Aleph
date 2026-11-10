# Classification & routing — IF3 (minimax-cn/MiniMax-M3.1-Flash)

| ID | Route | fix_shape | risk | note |
|---|---|---|---|---|
| IF3-01 | FIX-NOW | local-edit | medium | Reject plaintext events when encryption is configured without breaking URL verification. |
| IF3-02 | FIX-NOW | multi-site | medium | Never treat unresolved group card context as a DM. |
| IF3-03 | FIX-NOW | local-edit | medium | Preserve encryption and handle legacy entries without plaintext fallback. |
| IF3-04 | FIX-NOW | local-edit | low | Intersect allowed chats with existing access policy; keep empty list unrestricted. |
| IF3-05 | FIX-CAREFULLY | local-edit | low | Use the registry instance ID for both routing and reply delivery. |
| IF3-06 | FIX-CAREFULLY | multi-site | medium | Keep transport chat IDs intact; derive session scope separately. |
| IF3-07 | FIX-CAREFULLY | multi-site | medium | Deduplicate by event identity, not the replied-to card message. |
| IF3-08 | FIX-CAREFULLY | local-edit | low | Check provider envelope codes while preserving HTTP and request context. |
| IF3-09 | FIX-CAREFULLY | local-edit | medium | Do not settle the card until required final delivery succeeds. |
| IF3-10 | FIX-CAREFULLY | local-edit | medium | Keep one delivery path after fallback to preserve ordering and prefix. |
| IF3-11 | DEFER | multi-site | high | Avoid retrying the image after exposing the failed caption leg. |
| IF3-12 | DEFER | multi-site | high | Reject unsupported attachments instead of silently succeeding or resending media. |
| IF3-13 | DEFER | multi-site | high | Never enqueue media as an empty turn or discard required raw payload. |
| IF3-14 | FIX-CAREFULLY | multi-site | medium | Normalize phone identities consistently while preserving wildcard semantics. |
| IF3-15 | FIX-CAREFULLY | local-edit | low | Preserve NotConnected and SendFailed error variants exactly. |
| IF3-16 | FIX-CAREFULLY | local-edit | medium | Drive canonical pairing state from normal Connected events. |
| IF3-17 | FIX-CAREFULLY | multi-site | medium | Move vault I/O off the async runtime and propagate preparation errors. |
| IF3-18 | FIX-CAREFULLY | local-edit | low | Preserve image format and dimensions while moving work to spawn_blocking. |
| IF3-19 | DEFER | multi-site | medium | Bound retained groups and avoid caching messages rejected by policy. |
| IF3-20 | DEFER | multi-site | high | Join or cancel old lifecycle tasks before permitting replacement starts. |
| IF3-21 | FIX-CAREFULLY | local-edit | low | Evict old receipts incrementally without losing current-batch targets. |
