//! Component modules

pub mod connection_status;
pub mod forms;
pub mod layouts;
pub mod settings_sidebar;
pub mod sidebar;
pub mod ui;

// Re-export commonly used form components
pub use forms::{
    ErrorMessage, ErrorMessageDynamic, FormField, NumberInput, SaveButton, SelectInput,
    SettingsSection, SuccessMessage, SwitchInput, TextInput,
};

// Re-export settings sidebar
pub use settings_sidebar::SettingsSidebar;

// Re-export sidebar components
pub use sidebar::{Sidebar, SidebarItem};

// Re-export layouts
pub use layouts::SettingsLayout;
