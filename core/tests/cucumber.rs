//! Cucumber BDD Test Runner
//!
//! Run all tests: cargo test --test cucumber
//! Run specific feature: cargo test --test cucumber -- tests/features/config/
//! Run with tag: cargo test --test cucumber -- --tags @wip

mod world;
mod steps;

use cucumber::World;
use world::AlephWorld;

#[tokio::main]
async fn main() {
    AlephWorld::cucumber()
        .max_concurrent_scenarios(4)
        .run("tests/features")
        .await;
}
