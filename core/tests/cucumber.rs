//! Cucumber BDD Test Runner
//!
//! Run all tests: cargo test --test cucumber
//! Run specific feature: cargo test --test cucumber -- tests/features/config/
//! Run with tag: cargo test --test cucumber -- --tags @wip

mod world;
mod steps;

use cucumber::World;
use world::AlephWorld;

fn main() {
    // Spawn a thread with a larger stack (16 MB) to avoid stack overflow from
    // cucumber's recursive feature/step parsing combined with the large number
    // of registered step functions.
    let builder = std::thread::Builder::new()
        .name("cucumber-runner".to_string())
        .stack_size(16 * 1024 * 1024);

    let handle = builder
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to build tokio runtime")
                .block_on(async {
                    AlephWorld::cucumber()
                        .max_concurrent_scenarios(4)
                        .run("tests/features")
                        .await;
                });
        })
        .expect("Failed to spawn cucumber runner thread");

    handle.join().expect("Cucumber runner thread panicked");
}
