//! Component modules

pub mod agents_sidebar;
pub mod bottom_bar;
pub mod chat_sidebar;
pub mod connection_status;
pub mod dashboard_sidebar;
pub mod forms;
pub mod layouts;
pub mod markdown;
pub mod model_selector;
pub mod mode_sidebar;
pub mod settings_sidebar;
pub mod sidebar;
pub mod theme_toggle;
pub mod top_bar;
pub mod ui;

// Re-export layout components
pub use bottom_bar::{BottomBar, PanelMode};
pub use top_bar::TopBar;
pub use mode_sidebar::ModeSidebar;

// Re-export commonly used form components
pub use forms::{
    ErrorMessage, ErrorMessageDynamic, FormField, NumberInput, SaveButton, SelectInput,
    SettingsSection, SuccessMessage, SwitchInput, TextInput,
};

// Re-export sidebar components
pub use sidebar::SidebarItem;
