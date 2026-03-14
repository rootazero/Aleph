//! P8 — Real agent stubs.
//!
//! These are `#[ignore]` placeholders that require a live agent to run.
//! Gate on `ALEPH_TEST_AGENT` env var.

#[tokio::test]
#[ignore]
async fn real_agent_execution() {
    if std::env::var("ALEPH_TEST_AGENT").is_err() {
        eprintln!("Skipping: ALEPH_TEST_AGENT not set");
        return;
    }
    // TODO: Wire to actual agent_loop when integration is ready
    eprintln!("Real agent probe: placeholder");
}

#[tokio::test]
#[ignore]
async fn real_agent_timeout() {
    if std::env::var("ALEPH_TEST_AGENT").is_err() {
        eprintln!("Skipping: ALEPH_TEST_AGENT not set");
        return;
    }
    eprintln!("Real agent timeout probe: placeholder");
}
