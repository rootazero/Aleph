use std::collections::HashMap;
use crate::sync_primitives::Arc;

use crate::gateway::channel::{ChannelConfig, ChannelFactory, ChannelResult};

type ChannelFactoryFn = fn(ChannelConfig) -> ChannelResult<Arc<dyn ChannelFactory>>;

static PLUGINS: std::sync::LazyLock<std::sync::RwLock<HashMap<&'static str, ChannelFactoryFn>>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(HashMap::new()));

pub fn register(channel_type: &'static str, create: ChannelFactoryFn) {
    let mut guard = PLUGINS.write().unwrap();
    if guard.contains_key(channel_type) {
        panic!(
            "Duplicate ChannelFactory registration for type: {}",
            channel_type
        );
    }
    guard.insert(channel_type, create);
}

pub fn get_factory(channel_type: &str) -> Option<ChannelFactoryFn> {
    PLUGINS.read().unwrap().get(channel_type).copied()
}

pub fn channel_types() -> Vec<&'static str> {
    PLUGINS.read().unwrap().keys().copied().collect()
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

    fn test_creator(_config: ChannelConfig) -> ChannelResult<Arc<dyn ChannelFactory>> {
        Err(crate::gateway::channel::ChannelError::ConfigError(
            "test".into(),
        ))
    }

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
