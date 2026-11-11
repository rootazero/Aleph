# Adjudication — IF6 — 88apiclaude/claude-sonnet-5-5

| ID | Verdict | Reason | Corrected severity |
|---|---|---|---|
| IF6-01 | UPHELD | Line 146: `sender.contains(allowed)` substring match allows `boss@company.com.attacker.example` to pass | P0 |
| IF6-02 | UPHELD | Line 348: RFC822 fetch implicitly sets Seen; line 355: filter_map(r.ok()) drops errors; unconditional Seen STORE at end | P0 |
| IF6-03 | UPHELD | Lines 29, 16-30, 12-18: derived Debug on config structs directly exposes password/token fields | P0 |
| IF6-04 | UPHELD | Lines 328-346, 167-172: timestamp-only IDs collide across senders/conversations; Core dedup at 612-624 drops duplicates | P0 |
| IF6-05 | UPHELD | Lines 261, 214, 258, 330: factories hardcode type name as channel ID; no create_with_id override | P0 |
| IF6-06 | UPHELD | Line 124: markdown string becomes html without escaping; line 591: URL inserted into href without attribute escaping | P1 |
| IF6-07 | UPHELD | Line 306: nick captured before 433 handling; line 382-393: alt_nick requested but nick still used in convert_privmsg | P1 |
| IF6-08 | UPHELD | Line 230: send() returns Ok after queueing; line 430-433: write_all error only logs, no completion signal to sender | P1 |
| IF6-09 | UPHELD | Line 131: max retries hit causes break; line 171: result.is_err() only catches panic, not monitor's Ok(()) exhaustion | P1 |
| IF6-10 | UPHELD | Lines 90, 101, 94: capabilities.attachments=true; lines 178-200, 200-215, 229-238: send paths never read message.attachments | P1 |
| IF6-11 | UPHELD | Line 307-308: stdout.lock() and writeln!/flush are blocking std calls inside async fn pinned to runtime worker | P1 |
| IF6-12 | UPHELD | Line 216-218: std::thread spawned with stdin.lock(); line 278-283: stop doesn't join or cancel reader thread | P2 |
| IF6-13 | UPHELD | Line 603: outcome:None inserted; line 1007: prune keeps outcome=None forever; cancel during authorizer await leaves unreachable | P2 |
| IF6-14 | UPHELD | Line 663: pinned.is_some() uses Utc::now(), else uses stale captured now from line 980 after authorizer wait | P1 |
| IF6-15 | UPHELD | Line 331: SEARCH all UNSEEN; line 354-357: collect entire Vec before processing; no per-message or batch size limit | P2 |
| IF6-16 | UPHELD | Line 288: BufReader::new(reader).lines() has no per-line byte limit; remote can send unbounded line | P2 |
