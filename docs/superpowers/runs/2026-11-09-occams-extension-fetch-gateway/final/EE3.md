# Final verdicts — EE3

| ID | Verdict | Severity | Reason |
|---|---|---|---|
| EE3-01 | UPHELD | P0 | L188 reads first candidate, L202 parse_skill_file follows join("SKILL.md") symlink without containment check pregrant performs / Lines 188 and 202 select the first candidate then parse its `SKILL.md`; directory-only `operator_owned` checks do not establish the manifest file's containment. |
| EE3-02 | DOWNGRADE | P1 | L184-202 pregrant awaits load_candidates + parse_skill_file synchronously on executor thread, no spawn_blocking wrapper present / Lines 184-187 and 202 perform synchronous filesystem work inside an async path, but this is a responsiveness defect rather than a demonstrated permission bypass. |
| EE3-03 | UPHELD | P2 | L619-626 every strategy_store call creates tempdir().keep() then installs only first, abandoned directories accumulate |
| EE3-04 | UPHELD | P3 | L115 reads local t.request.metadata before skill_read, L122 reads same unchanged local object, comparison is identity tautology / Lines 115-122 compare the same local `t` values before and after an unrelated call; the assertion cannot observe any authorization effect. |
