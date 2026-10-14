#!/bin/bash
cd /d/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256
export CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_CODEGEN_UNITS=256 CARGO_PROFILE_TEST_INCREMENTAL=false CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0 -C codegen-units=256'
L=.analysis/browser-live-runtime
run() { n=$1; shift
  py -B qa/browser_live/check_memory.py || { echo "$n memory BLOCK"; return; }
  cargo test -p alephcore --lib "$@" -- --test-threads=1 > $L/gw-verify-$n.log 2>&1
  echo "$n exit $?"; }
run 2-lane gateway::lane::
run 3-continuation every_epoch_bump_or_content_wipe_reaches_a_side_session_retirement
run 4-events-carveout cli_carve_out
run 5-visibility-census every_ambient_owner_call_site_is_in_the_census
run 6-visibility-census2 the_census_names_no_file_that_stopped_using_it
run 7-sans-caller run_principal_sans_caller
