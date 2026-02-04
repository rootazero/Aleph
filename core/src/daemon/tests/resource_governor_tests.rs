#[cfg(test)]
mod tests {
    use crate::daemon::*;

    #[tokio::test]
    async fn test_resource_governor_creation() {
        let governor = ResourceGovernor::new(
            ResourceLimits {
                cpu_threshold: 20.0,
                mem_threshold: 512 * 1024 * 1024,
                battery_threshold: 20.0,
            }
        );

        assert_eq!(governor.limits().cpu_threshold, 20.0);
    }

    #[tokio::test]
    async fn test_resource_governor_check() {
        let governor = ResourceGovernor::new(ResourceLimits::default());
        let decision = governor.check().await;

        // Should return either Proceed or Throttle
        assert!(matches!(decision, Ok(GovernorDecision::Proceed) | Ok(GovernorDecision::Throttle)));
    }
}
