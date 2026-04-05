use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

use crate::gateway::channel::{ChannelConfig, ChannelFactory, ChannelResult};

type FactoryFn = fn(ChannelConfig) -> ChannelResult<Arc<dyn ChannelFactory>>;

static PLUGINS: LazyLock<
    RwLock<HashMap<&'static str, FactoryFn>>,
    fn() -> RwLock<HashMap<&'static str, FactoryFn>>,
> = LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn register(channel_type: &'static str, factory: FactoryFn) {
    PLUGINS.write().unwrap().insert(channel_type, factory);
}

pub fn channel_types() -> Vec<&'static str> {
    PLUGINS.read().unwrap().keys().copied().collect()
}

pub fn create(channel_type: &str, config: ChannelConfig) -> ChannelResult<Arc<dyn ChannelFactory>> {
    let factory = PLUGINS
        .read()
        .unwrap()
        .get(channel_type)
        .copied()
        .ok_or_else(|| {
            crate::gateway::channel::ChannelError::ConfigError(format!(
                "No plugin for channel type: {channel_type}"
            ))
        })?;
    factory(config)
}
