# Classification & routing — GX1 (minimax-cn/MiniMax-M3.1-Flash)

| ID | Route | fix_shape | risk | note |
|---|---|---|---|---|
| GX1-01 | FIX-NOW | multi-site | medium | Deny missing identities without weakening existing session visibility or not-found behavior. |
| GX1-02 | FIX-NOW | local-edit | medium | Sandbox active documents without breaking images, media, or shared header policy. |
| GX1-03 | FIX-NOW | local-edit | low | Never log raw frame bytes; retain only non-sensitive metadata. |
| GX1-04 | FIX-NOW | multi-site | high | Revalidate live authority for dispatch, waiters, and cache hits. |
| GX1-05 | FIX-NOW | local-edit | low | Return a controlled MIME fallback without altering attachment bytes. |
| GX1-06 | FIX-CAREFULLY | local-edit | medium | Keep vault locking and atomic commit entirely inside one blocking closure. |
| GX1-07 | FIX-CAREFULLY | multi-site | medium | Preserve update ordering; never await storage while holding the connections lock. |
| GX1-08 | DEFER | multi-site | high | Reserve both global and per-IP capacity before upgrade, releasing failed slots. |
| GX1-09 | DEFER | multi-site | medium | Cancel and join every maintenance task on startup failure and shutdown. |
| GX1-10 | DEFER | multi-site | medium | Bound approvals per connection and cancel owned waiters during teardown. |
| GX1-11 | FIX-CAREFULLY | local-edit | none | Assert executable delivery wiring, never documentation-only occurrences. |
