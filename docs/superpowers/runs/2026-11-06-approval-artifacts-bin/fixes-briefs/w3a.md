# W3A brief — approval fixes (wave 3)

Contract: `../BRIEF_FIX.md` (same directory). Worktree: /home/zou/data/workspace/Aleph-wt-fix-wave3a (branch fix-wave3a).
Report: `/home/zou/data/workspace/Aleph-wt-fix-wave3a/.review/rd-2026-11-06-approval-artifacts-bin/fixes/w3a.md` (uncommitted, one section per ID).

## W3A-DC-01 [P2] Originator callback authorization is not atomic across approval ID reuse
- location: src/approval/callback_sink.rs:64, src/exec/manager.rs:694-729 (HEAD)
- kind: partial-fix
- finding: The callback sink checks the live record's `originator_user_id` under one `get_pending` snapshot, then later calls `resolve_for_channel`; the manager's locked path rechecks liveness and operator-only status but never rechecks the originator. Because registration rejects duplicate IDs only while the old entry is present and permits the same ID after resolution or retirement, a delayed callback can pass as Alice for the old Alice record, have that record replaced by Bob's record with the same ID, and then resolve Bob's approval (this also bypasses the new no-originator refusal).
- suggested-fix: Move the originator comparison into the manager's write-locked channel-resolution path and require the supplied channel user to match the CURRENT record's originator, rejecting `None`; have the callback path use that atomic check and derive its effective decision from the same locked record.

## W3A-DC-02 [P2] URL case folding changes case-sensitive path and non-URL text
- location: src/approval/config.rs:559 and src/approval/config.rs:759-795 (HEAD)
- kind: over-reach
- finding: `decide` applies `canonicalize_url_case` to every action target, and the helper repeatedly searches the remaining suffix for `://` instead of stopping after the first URL authority. A target such as `https://example.com/a://BLAH` is rewritten to contain `/a://blah`, so case-sensitive path/query text can match a different rule; non-URL targets containing `://` are also folded.
- suggested-fix: Restrict folding to action types whose targets are URLs, and parse only the actual scheme/authority span so path, query, fragment and embedded command text are copied unchanged. Add a regression case containing `://` after the authority.

Scope: src/approval/callback_sink.rs, src/approval/config.rs, src/exec/manager.rs only.
Read the current HEAD code before editing — wave-1 and wave-2 fixes are already merged.
