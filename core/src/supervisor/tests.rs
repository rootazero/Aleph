//! Integration tests for PtySupervisor.
//!
//! These tests use real PTY processes to verify the supervisor works correctly.

use crate::supervisor::{ClaudeSupervisor, SupervisorConfig, SupervisorEvent};
use std::time::Duration;

/// Test spawning a simple echo command.
#[test]
fn test_spawn_echo() {
    let config = SupervisorConfig::new("/tmp")
        .with_command("echo")
        .with_args(vec!["Hello from PTY".to_string()]);

    let mut supervisor = ClaudeSupervisor::new(config);
    let mut rx = supervisor.spawn().expect("Failed to spawn");

    // Collect events with timeout
    let mut outputs = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);

    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(SupervisorEvent::Output(text)) => {
                outputs.push(text);
            }
            Ok(SupervisorEvent::Exited(_)) => break,
            Ok(_) => {}
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
        }
    }

    // Verify we got the expected output
    let combined = outputs.join("");
    assert!(
        combined.contains("Hello from PTY"),
        "Expected 'Hello from PTY' in output, got: {:?}",
        outputs
    );
}

/// Test writing input to a cat process.
#[test]
fn test_write_to_cat() {
    let config = SupervisorConfig::new("/tmp").with_command("cat");

    let mut supervisor = ClaudeSupervisor::new(config);
    let mut rx = supervisor.spawn().expect("Failed to spawn");

    // Write input
    supervisor
        .writeln("Test input line")
        .expect("Failed to write");

    // Collect output
    let mut outputs = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);

    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(SupervisorEvent::Output(text)) => {
                let found = text.contains("Test input line");
                outputs.push(text);
                if found {
                    break;
                }
            }
            Ok(_) => {}
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
        }
    }

    let combined = outputs.join("");
    assert!(
        combined.contains("Test input line"),
        "Expected 'Test input line' in output, got: {:?}",
        outputs
    );
}

/// Test that is_running reflects actual state.
#[test]
fn test_is_running_state() {
    let config = SupervisorConfig::new("/tmp")
        .with_command("echo")
        .with_args(vec!["quick".to_string()]);

    let mut supervisor = ClaudeSupervisor::new(config);
    assert!(
        !supervisor.is_running(),
        "Should not be running before spawn"
    );

    let mut rx = supervisor.spawn().expect("Failed to spawn");

    // Give it a moment to start
    std::thread::sleep(Duration::from_millis(50));

    // Wait for exit
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(SupervisorEvent::Exited(_)) => break,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    }

    // After echo exits, should eventually show not running
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !supervisor.is_running(),
        "Should not be running after process exits"
    );
}
