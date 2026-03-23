include!(concat!(env!("OUT_DIR"), "/i18n/mod.rs"));

// Re-export all items from the generated i18n module for ergonomic usage:
// `use crate::i18n::*;` in any file makes `t!`, `use_i18n`, `Locale`,
// `I18nContextProvider`, etc. available directly.
pub use i18n::*;
