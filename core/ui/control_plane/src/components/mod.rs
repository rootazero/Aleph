//! Component modules

pub mod connection_status;
pub mod forms;
pub mod sidebar;
pub mod ui;

// Re-export commonly used form components
pub use forms::{
    ErrorMessage, ErrorMessageDynamic, FormField, NumberInput, SaveButton, SelectInput,
    SettingsSection, SuccessMessage, SwitchInput, TextInput,
};
