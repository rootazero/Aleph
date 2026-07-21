use crate::sync_primitives::Arc;
use std::collections::HashMap;

use crate::gateway::channel::{ChannelConfig, ChannelError, ChannelFactory, ChannelResult};

type ChannelFactoryFn = fn(ChannelConfig) -> ChannelResult<Arc<dyn ChannelFactory>>;

static PLUGINS: std::sync::LazyLock<
    crate::sync_primitives::RwLock<HashMap<&'static str, ChannelFactoryFn>>,
> = std::sync::LazyLock::new(|| crate::sync_primitives::RwLock::new(HashMap::new()));

// Duplicate registration is a static wiring bug (each channel registers its
// factory exactly once at startup). Return an error instead of panicking so
// library callers remain in control.
pub fn register(channel_type: &'static str, create: ChannelFactoryFn) -> ChannelResult<()> {
    let mut guard = PLUGINS.write().unwrap_or_else(|e| e.into_inner());
    if guard.contains_key(channel_type) {
        return Err(ChannelError::ConfigError(format!(
            "Duplicate ChannelFactory registration for type: {channel_type}"
        )));
    }
    guard.insert(channel_type, create);
    Ok(())
}

pub fn get_factory(channel_type: &str) -> Option<ChannelFactoryFn> {
    PLUGINS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(channel_type)
        .copied()
}

pub fn channel_types() -> Vec<&'static str> {
    let mut types = PLUGINS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .copied()
        .collect::<Vec<_>>();
    types.sort_unstable();
    types
}

pub fn create(channel_type: &str, config: ChannelConfig) -> ChannelResult<Arc<dyn ChannelFactory>> {
    let creator = get_factory(channel_type).ok_or_else(|| {
        crate::gateway::channel::ChannelError::ConfigError(format!(
            "No plugin registered for channel type: {channel_type}"
        ))
    })?;
    creator(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unknown_channel_type() {
        let result = create(
            "nonexistent_channel",
            ChannelConfig {
                id: "test".into(),
                channel_type: "nonexistent".into(),
                enabled: true,
                config: serde_json::json!({}),
            },
        );
        assert!(result.is_err());
    }
}
