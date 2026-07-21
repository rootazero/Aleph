//! Component modules

pub mod agents_sidebar;
pub mod approval_card;
pub mod ask_user_card;
pub mod boot_check_gate;
pub mod chat_sidebar;
pub mod command_palette;
pub mod connection_status;
pub mod dashboard_sidebar;
pub mod directory_browser;
pub mod exec_tier_labels;
pub mod mode_labels;
pub mod extensions;
pub mod forms;
pub mod inspector;
pub mod json_schema_form;
pub mod layout_toggle;
pub mod layouts;
pub mod markdown;
pub mod mode_sidebar;
pub mod model_picker;
pub mod nav_menu;
pub mod notification_center;
pub mod provider_badge;
pub mod provider_key_field;
pub mod provider_row_card;
pub mod service_blocking_gate;
pub mod settings_sidebar;
pub mod sidebar;
pub mod team_participants;
pub mod team_task_strip;
pub mod theme_toggle;
pub mod token_wall;
pub mod tool_card;
pub mod ui;
pub mod workspace_panel;

// Re-export layout components
pub use mode_sidebar::{ModeSidebar, PanelMode};

// Re-export commonly used form components
pub use forms::{
    ErrorMessage, ErrorMessageDynamic, FormField, NumberInput, SaveButton, SelectInput,
    SettingsSection, SuccessMessage, SwitchInput, TextInput,
};

// Re-export sidebar components
pub use sidebar::SidebarItem;
