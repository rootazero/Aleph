# Gateway off-loop verification (read-only run)
Env actually effective (rustc cmdline inspected): -C codegen-units=256, -C debuginfo=0, embed-bitcode=no, strip=debuginfo, CARGO_BUILD_JOBS=1.
Build: alephcore lib test, 13m30s, no OOM/timeout. Memory gate ALLOW (>=13.7GB free) before each run.
Path audit: selected tests use SecurityStore::in_memory / in-memory managers / read-only source scans (read_dir, read_to_string); no home_dir, keyring, env mutation, remove_*.
All with --test-threads=1, cargo test -p alephcore --lib <filter>:
1 gateway::server::connection::  27 passed 0 failed (auth 19, off_loop 8 incl. the_connection_loop_uses_the_off_loop_set, shutdown_aborts_active_and_drops_queued_work)  log gw-verify-1-connection.log
2 gateway::lane::  21 passed 0 failed
3 every_epoch_bump_or_content_wipe_reaches_a_side_session_retirement  1 passed
4 cli_carve_out  1 passed (the_cli_carve_out_delivers_everything_an_unsubscribed_socket_did_but_reasoning)
5 every_ambient_owner_call_site_is_in_the_census 1 passed; 6 the_census_names_no_file_that_stopped_using_it 1 passed; 7 run_principal_sans_caller 1 passed
Not run: broad gateway suite, browser::live tests, method_census, native/AppContainer/admin tests, clippy/fmt.
