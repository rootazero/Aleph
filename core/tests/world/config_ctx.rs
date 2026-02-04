//! Configuration test context

use alephcore::{Config, MemoryConfig, BehaviorConfig, ShortcutsConfig};

#[derive(Debug, Default)]
pub struct ConfigContext {
    pub config: Option<Config>,
    pub memory_config: Option<MemoryConfig>,
    pub behavior_config: Option<BehaviorConfig>,
    pub shortcuts_config: Option<ShortcutsConfig>,
    pub validation_result: Option<Result<(), String>>,
    pub parse_result: Option<Result<Config, String>>,
}
