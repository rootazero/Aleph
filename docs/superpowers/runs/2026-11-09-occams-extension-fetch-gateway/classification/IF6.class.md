# Classification & routing — IF6 (minimax-cn/MiniMax-M3.1-Flash)

| ID | Route | fix_shape | risk | note |
|---|---|---|---|---|
| IF6-01 | FIX-NOW | local-edit | low | Match domain entries by parsed exact domain; preserve full-mailbox and empty-list behavior. |
| IF6-02 | FIX-NOW | multi-site | high | Use PEEK and acknowledge only successfully published or explicitly discarded messages. |
| IF6-03 | FIX-NOW | multi-site | low | Redact every secret in Debug output while preserving existing serde configuration formats. |
| IF6-04 | FIX-NOW | multi-site | medium | Keep Core deduplication enabled while generating stable, collision-resistant adapter message IDs. |
| IF6-05 | FIX-NOW | multi-site | medium | Honor configured instance IDs across all four factories without changing legacy create defaults. |
| IF6-06 | FIX-NOW | local-edit | low | Escape text, inline code, and link attributes without escaping generated HTML tags. |
| IF6-07 | FIX-NOW | local-edit | medium | Track the confirmed nickname and advance retries without changing mention or allowlist rules. |
| IF6-08 | FIX-NOW | local-edit | medium | Await actual write completion and never blindly retry a possibly partially written command. |
| IF6-09 | FIX-NOW | multi-site | medium | Map retry exhaustion to Error while preserving intentional shutdown as Disconnected. |
| IF6-10 | FIX-NOW | multi-site | low | Advertise only implemented features and reject unsupported attachments instead of reporting success. |
| IF6-11 | FIX-NOW | local-edit | medium | Perform blocking stdout writes off-worker while preserving ordering and error propagation. |
| IF6-12 | DEFER | multi-site | high | Do not let a stale stdin reader consume the restarted instance's first line. |
| IF6-13 | DEFER | multi-site | high | Avoid duplicate charges while ensuring cancelled reservations have deterministic terminal outcomes. |
| IF6-14 | FIX-NOW | local-edit | low | Recheck current expiry after authorization, preserving explicit submit_at caller-time semantics. |
| IF6-15 | FIX-CAREFULLY | local-edit | medium | Bound both fetched messages and retained body bytes while leaving later mail unseen. |
| IF6-16 | FIX-CAREFULLY | local-edit | medium | Enforce the frame limit before allocation without changing normal IRC message handling. |
