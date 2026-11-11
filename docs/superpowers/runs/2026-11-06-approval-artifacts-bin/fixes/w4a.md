# Wave 4a fix report

Commit: `29bfda7ec` (`fix(migrate): fail closed on migration inconsistencies`)

## B13-10 [P1] Filesystem-only sessions can still produce a successful verification
- verdict: fixed
- files: src/bin/aleph-migrate/verify.rs:91-165, src/bin/aleph-migrate/verify.rs:998-1009
- change: Verification now builds the expected session-directory set from database session IDs and compares it with the actual top-level directories in both directions. Every missing or filesystem-only directory increments the error count and is printed with its concrete session ID or directory name, so a count-only or key mismatch cannot produce SUCCESS.
- risk: Existing exports with unindexed directories now fail verification even when their file counts happen to match. Non-UTF-8 directory names are reported lossily rather than aborting the remaining checks.

## B13-11 [P1] FTS verification does not check transcript coverage or stale rows
- verdict: fixed
- files: src/bin/aleph-migrate/verify.rs:831-917
- change: FTS verification now reads every stored `(session_id, content)` row and compares it with each database session's filesystem transcript. Missing transcripts, unexpected session IDs, duplicate rows, and stale content are all concrete errors; an empty index therefore fails whenever transcripts exist.
- risk: Exact content comparison intentionally rejects stale or manually normalized FTS content that the old probe query accepted. Transcript contents are held in memory during this comparison, increasing peak memory for large migrations.

## B13-12 [P0] Orphan cleanup deletes attachments owned by sessions whose directories are missing
- verdict: fixed
- files: src/bin/aleph-migrate/verify.rs:471-500, src/bin/aleph-migrate/verify.rs:756-793
- change: The protected set now includes every attachment hash and path referenced by `metadata.db`, independently of session manifests. Sweeping is allowed only when manifests are complete and the entire verification has zero errors; missing session directories or any earlier mismatch therefore disable deletion, and database paths still protect files whose bytes no longer match the stored hash.
- risk: `--remove-orphans` is deliberately more conservative and will not remove even genuine orphans until all verification errors are repaired. No live database-referenced attachment is intentionally removed.

## B13-13 [P1] Identical bytes with different MIME types cannot be represented by the attachment index
- verdict: partially-fixed
- files: src/bin/aleph-migrate/verify.rs:478-478, src/bin/aleph-migrate/verify.rs:618-638, src/bin/aleph-migrate/verify.rs:694-736, src/bin/aleph-migrate/verify.rs:800-804
- change: Verification now records hash/path/MIME identities across manifests, reports conflicting identities with the concrete hash, and checks indexed MIME type as well as path and size. The destructive index collision is consequently fail-closed instead of being reduced to a generic successful verification.
- risk: Distinct logical attachments that share bytes now reliably fail verification, as intended, but the database schema still cannot represent both rows.
- skipped-reason: The root fix requires changing the `attachments` schema and insertion key in `index.rs`; this wave explicitly limited edits to `verify.rs`, so no producer/schema change was made.

## B13-14 [P1] “Metadata consistency” accepts values that disagree with the database
- verdict: fixed
- files: src/bin/aleph-migrate/verify.rs:252-415
- change: The check now loads all exported session metadata fields from `sessions` and compares session ID, agent, workspace, timestamps, counts, compression status, and tags with each JSON file. Every mismatch is reported with expected and actual values and makes verification fail; `--fix` continues to treat the database as the repair source.
- risk: Older metadata files that omitted optional-but-indexed fields now require repair instead of passing. One repair decrements one session-level error even when several fields for that session were listed.

## B13-15 [P1] Metadata auto-fix splits legal tags containing commas
- verdict: fixed
- files: src/bin/aleph-migrate/verify.rs:919-967
- change: Metadata regeneration no longer splits comma-containing database text into tags. It preserves an unambiguous single tag, maps the established empty representation to an empty array, and refuses regeneration with session context when comma text makes tag boundaries ambiguous.
- risk: `--fix` now fails instead of repairing metadata for either legacy multi-tag sessions or a single comma-containing tag, because those cases cannot be distinguished in the current database encoding. This avoids silently inventing tag boundaries.

## B13-16 [P1] Export succeeds after dropping source rows with null timestamps
- verdict: partially-fixed
- files: src/bin/aleph-migrate/verify.rs:133-136, src/bin/aleph-migrate/verify.rs:168-198
- change: When `--original-db` is supplied, verification detects the timestamp column used by export and reports every source row with a session ID but a null timestamp, including its row ID and session ID. Such a lossy export can no longer pass this verification path.
- risk: Original databases containing null-timestamp rows now fail verification even if operators intentionally excluded their content. Without `--original-db`, there is no source evidence from which verification can detect the dropped rows.
- skipped-reason: Preventing `export` itself from succeeding requires changing `export.rs`, which is outside this wave's permitted edit scope; only verification-side detection was added.

## B13-17 [P2] Missing attachment counts silently disable attachment indexing
- verdict: partially-fixed
- files: src/bin/aleph-migrate/verify.rs:390-410, src/bin/aleph-migrate/verify.rs:531-588
- change: Message, tool, and attachment counts must now match database values and be nonnegative integers, while attachment verification already rejects a zero count paired with a nonempty manifest. Missing or wrongly typed counts therefore make verification fail rather than silently accepting an attachment-free index.
- risk: Legacy metadata with omitted counts now requires repair. The index command itself still defaults malformed counts to zero when run independently.
- skipped-reason: The root producer fix belongs to `index_single_session` / `index_attachments` in `index.rs`; this wave limited edits to `verify.rs`, so only fail-closed verification was implemented.

## Validation and scope
- Ran `git diff --check`; committed only `src/bin/aleph-migrate/verify.rs` on `fix-w4a`.
- Did not run cargo, clippy, rustfmt, or tests, as required by the contract. Did not change the CLI, the attachment index schema, export behavior, or indexing behavior.
