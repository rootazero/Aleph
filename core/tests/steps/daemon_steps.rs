//! Step definitions for daemon features

use crate::world::{AlephWorld, DaemonContext};
use alephcore::daemon::{
    DaemonCli, DaemonCommand, DaemonConfig, DaemonEvent, DaemonEventBus, DaemonStatus,
    GovernorDecision, RawEvent, ResourceGovernor, ResourceLimits, ServiceManager, ServiceStatus,
};
use chrono::Utc;
use clap::Parser;
use cucumber::{given, then, when};

// ═══ Event Bus Steps ═══

#[given(expr = "an event bus with capacity {int}")]
async fn given_event_bus(w: &mut AlephWorld, capacity: i32) {
    let ctx = w.daemon.get_or_insert_with(DaemonContext::default);
    ctx.event_bus = Some(DaemonEventBus::new(capacity as usize));
}

#[given("a subscriber to the event bus")]
async fn given_subscriber(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let bus = ctx.event_bus.as_ref().expect("Event bus not initialized");
    ctx.receivers.push(bus.subscribe());
}

#[given(expr = "{int} subscribers to the event bus")]
async fn given_multiple_subscribers(w: &mut AlephWorld, count: i32) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let bus = ctx.event_bus.as_ref().expect("Event bus not initialized");
    for _ in 0..count {
        ctx.receivers.push(bus.subscribe());
    }
}

#[when("I send a heartbeat event")]
async fn when_send_heartbeat(w: &mut AlephWorld) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let bus = ctx.event_bus.as_ref().expect("Event bus not initialized");
    let event = DaemonEvent::Raw(RawEvent::Heartbeat {
        timestamp: Utc::now(),
    });
    bus.send(event).unwrap();
}

#[then("the subscriber should receive a heartbeat event")]
async fn then_receive_heartbeat(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let receiver = ctx.receivers.first_mut().expect("No subscribers");
    let event = receiver.recv().await.unwrap();
    assert!(matches!(
        event,
        DaemonEvent::Raw(RawEvent::Heartbeat { .. })
    ));
}

#[then("all subscribers should receive a heartbeat event")]
async fn then_all_receive_heartbeat(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    for receiver in ctx.receivers.iter_mut() {
        let event = receiver.recv().await.unwrap();
        assert!(matches!(
            event,
            DaemonEvent::Raw(RawEvent::Heartbeat { .. })
        ));
    }
}

// ═══ CLI Steps ═══

#[when(expr = "I parse CLI arguments {string}")]
async fn when_parse_cli(w: &mut AlephWorld, args_str: String) {
    let ctx = w.daemon.get_or_insert_with(DaemonContext::default);
    let args: Vec<&str> = args_str.split_whitespace().collect();
    ctx.cli_parse_result = Some(
        DaemonCli::try_parse_from(args)
            .map(|cli| {
                // Verify it's an Install command
                assert!(matches!(cli.command, DaemonCommand::Install));
            })
            .map_err(|e| e.to_string()),
    );
}

#[then("the CLI parsing should succeed")]
async fn then_cli_success(w: &mut AlephWorld) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let result = ctx
        .cli_parse_result
        .as_ref()
        .expect("No CLI parse attempted");
    assert!(result.is_ok(), "CLI parsing failed: {:?}", result);
}

#[then(expr = "the command should be Install")]
async fn then_command_install(_w: &mut AlephWorld) {
    // The parsing already validated this matches Install command in when_parse_cli
    // This step is for documentation/readability
}

// ═══ Service Manager Steps ═══

#[given("a mock service manager")]
async fn given_mock_service(w: &mut AlephWorld) {
    // Mock service manager is created inline for the test
    let _ = w.daemon.get_or_insert_with(DaemonContext::default);
}

#[when("I query the service status")]
async fn when_query_service_status(w: &mut AlephWorld) {
    // Create and test mock inline
    struct MockService;

    #[async_trait::async_trait]
    impl ServiceManager for MockService {
        async fn install(&self, _config: &DaemonConfig) -> alephcore::daemon::Result<()> {
            Ok(())
        }
        async fn uninstall(&self) -> alephcore::daemon::Result<()> {
            Ok(())
        }
        async fn start(&self) -> alephcore::daemon::Result<()> {
            Ok(())
        }
        async fn stop(&self) -> alephcore::daemon::Result<()> {
            Ok(())
        }
        async fn status(&self) -> alephcore::daemon::Result<DaemonStatus> {
            Ok(DaemonStatus::Unknown)
        }
        async fn service_status(&self) -> alephcore::daemon::Result<ServiceStatus> {
            Ok(ServiceStatus::NotInstalled)
        }
    }

    let service: Box<dyn ServiceManager> = Box::new(MockService);
    let result = service.service_status().await;

    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    ctx.cli_parse_result = Some(result.map(|_| ()).map_err(|e| e.to_string()));
}

#[then("the query should succeed")]
async fn then_query_success(w: &mut AlephWorld) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let result = ctx
        .cli_parse_result
        .as_ref()
        .expect("No query attempted");
    assert!(result.is_ok(), "Service status query failed: {:?}", result);
}

// ═══ Resource Governor Steps ═══

#[given(expr = "a resource governor with CPU threshold {float}")]
async fn given_governor_custom(w: &mut AlephWorld, cpu_threshold: f32) {
    let ctx = w.daemon.get_or_insert_with(DaemonContext::default);
    ctx.governor = Some(ResourceGovernor::new(ResourceLimits {
        cpu_threshold,
        mem_threshold: 512 * 1024 * 1024,
        battery_threshold: 20.0,
    }));
}

#[given("a resource governor with default limits")]
async fn given_governor_default(w: &mut AlephWorld) {
    let ctx = w.daemon.get_or_insert_with(DaemonContext::default);
    ctx.governor = Some(ResourceGovernor::new(ResourceLimits::default()));
}

#[when("I check the governor decision")]
async fn when_check_governor(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let governor = ctx.governor.as_ref().expect("Governor not initialized");
    ctx.governor_decision = Some(governor.check().await.map_err(|e| e.to_string()));
}

#[then(expr = "the governor CPU threshold should be {float}")]
async fn then_governor_cpu_threshold(w: &mut AlephWorld, expected: f32) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let governor = ctx.governor.as_ref().expect("Governor not initialized");
    assert!(
        (governor.limits().cpu_threshold - expected).abs() < 0.001,
        "Expected CPU threshold {}, got {}",
        expected,
        governor.limits().cpu_threshold
    );
}

#[then("the decision should be Proceed or Throttle")]
async fn then_decision_proceed_or_throttle(w: &mut AlephWorld) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let decision = ctx.governor_decision.as_ref().expect("No decision made");
    match decision {
        Ok(GovernorDecision::Proceed) | Ok(GovernorDecision::Throttle) => {}
        other => panic!("Expected Proceed or Throttle, got {:?}", other),
    }
}
