## DC-01 [P2] Originator callback authorization is not atomic across approval ID reuse
- location: `src/approval/callback_sink.rs:64` and `src/exec/manager.rs:694-729` in HEAD
- kind: partial-fix
- finding: The callback sink checks the live record's `originator_user_id` under one `get_pending` snapshot, then later calls `resolve_for_channel`; the manager's locked path rechecks liveness and operator-only status but never rechecks the originator. Because registration rejects duplicate IDs only while the old entry is present and permits the same ID after resolution or retirement, a delayed callback can pass as Alice for the old Alice record, have that record replaced by Bob's record with the same ID, and then resolve Bob's approval (the same race also bypasses the new no-originator refusal).
- suggested-fix: Move the originator comparison into the manager's write-locked channel-resolution path and require the supplied channel user to match the current record's originator, rejecting `None`; have the callback path use that atomic check and derive its effective decision from the same locked record.

## DC-02 [P2] URL case folding changes case-sensitive path and non-URL text
- location: `src/approval/config.rs:559` and `src/approval/config.rs:759-795` in HEAD
- kind: over-reach
- finding: `decide` applies `canonicalize_url_case` to every action target, and the helper repeatedly searches the remaining suffix for `://` instead of stopping after the first URL authority. A target such as `https://example.com/a://BLAH` is rewritten to contain `/a://blah`, so text in a path or query that is supposed to remain case-sensitive can match a different rule; command or other non-URL targets containing `://` are also rewritten. This exceeds the intended scheme/host normalization and can change both allowlist and blocklist decisions.
- suggested-fix: Restrict folding to action types whose targets are URLs, and parse only the actual scheme/authority span so path, query, fragment, and embedded command text are copied unchanged. Add a regression case containing `://` after the authority and a non-URL target with mixed-case suffix text.

## Summary
- hunks-reviewed: 66
- concerns: 2
- verdict: concerns-found
