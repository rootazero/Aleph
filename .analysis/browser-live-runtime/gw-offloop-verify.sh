#!/bin/bash
cd /d/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256
export CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_CODEGEN_UNITS=256 CARGO_PROFILE_TEST_INCREMENTAL=false CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0 -C codegen-units=256'
L=.analysis/browser-live-runtime
echo "ENV: jobs=$CARGO_BUILD_JOBS dbg=$CARGO_PROFILE_DEV_DEBUG tdbg=$CARGO_PROFILE_TEST_DEBUG cgu=$CARGO_PROFILE_TEST_CODEGEN_UNITS rf=$RUSTFLAGS"
py -B qa/browser_live/check_memory.py || exit 90
cargo test -p alephcore --lib gateway::server::connection:: -- --test-threads=1 > $L/gw-verify-1-connection.log 2>&1
echo "step1 exit $?"
