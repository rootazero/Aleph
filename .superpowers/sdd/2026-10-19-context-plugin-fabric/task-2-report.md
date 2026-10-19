# Task 2 implementation report — host grants and registry

Status: implementation and independent review complete; latest combined cargo verification pending. This report does not claim unexecuted tests passed.

## Changes / 变更

- `src/extension/capability.rs`: native host-issued `ContextGrant`, explicit approval, private authority fields/no deserializer; scope/side-effect/expiry checks. A canonical descriptor fingerprint binds the grant to the complete descriptor actually approved by the host.
- `src/extension/registry/types.rs`: one descriptor per registration, grants keyed by authenticated invoking session, no first-grant compatibility copy; bundle/lifecycle identity and explicit registry errors/stats.
- `src/extension/registry/plugin_registry/{mod,context,context_tests}.rs`: single registry authority; atomic bundles, conflict checks per invoking session, stable eligible-key selection, cleanup and per-session revocation; active owner/grant checks shared by selection and invocation.
- Audit uses a 256-entry VecDeque, saturating lifetime total and retained count. Rejections retain the host approval reason and latest decision; payloads are excluded.

## Review / 审查

Independent reviewer `5a1dc562-b6b9-44b` approved after the descriptor-binding fix. The earlier three issues (multiple sessions, global owner conflicts, unbounded audit) are closed. A further descriptor-mutation test revealed missing consent binding; host_issue now hashes the full descriptor, invocation rechecks it, and DescriptorChanged is fail-closed.

## Verification / 验证

Coordinator runs the combined Task 1 follow-up / Task 2 / Task 4 batch with `CARGO_BUILD_JOBS=4 CARGO_PROFILE_DEV_DEBUG=0`. The guard checks MemAvailable >=4GiB before each cargo invocation.

The first compile found E0063 in a fold-ledger test constructor, E0502 when returning an admitted descriptor borrow while appending audit, and E0716 in two temporary audit snapshots. Those have been fixed; the retry is still compiling at this report's creation. Logs are under the same SDD directory (`validation-2-4*.log`). Final results will be appended before acceptance.

## Remaining scope / 未做

No runtime transport or ContextFabric production mounting in Task 2; Tasks 3/5/6 own those integrations. No claim of native Node/Pi plugin loading. No full suite or final clippy result for this revision yet.
