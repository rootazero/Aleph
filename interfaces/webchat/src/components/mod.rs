//! Component modules

pub mod agents_sidebar;
pub mod api_key_input;
pub mod boot_check_gate;
pub mod chat_sidebar;
pub mod command_palette;
pub mod connection_status;
pub mod dashboard_sidebar;
pub mod directory_browser;
pub mod forms;
pub mod layouts;
pub mod markdown;
pub mod mode_sidebar;
pub mod nav_menu;
pub mod notification_center;
pub mod service_blocking_gate;
pub mod settings_sidebar;
pub mod sidebar;
pub mod theme_toggle;
pub mod ui;

// Re-export layout components
pub use mode_sidebar::{ModeSidebar, PanelMode};

// Re-export commonly used form components
pub use forms::{
    ErrorMessage, ErrorMessageDynamic, FormField, NumberInput, SaveButton, SelectInput,
    SettingsSection, SuccessMessage, SwitchInput, TextInput,
};

// Re-export sidebar components
pub use sidebar::SidebarItem;
