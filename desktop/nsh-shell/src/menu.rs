//! The app's commands in the tray menu and, on macOS, the menu bar.

#[cfg(target_os = "macos")]
use nsh_core::desktop::MenuRole;
use nsh_core::element::desktop::{quit, MenuItem};
use std::rc::Rc;

pub(crate) type Command = Rc<dyn Fn()>;

/// What the menus can do; [`crate::App`] builds each command.
pub(crate) struct Commands {
    pub(crate) show: Command,
    pub(crate) open_in_browser: Command,
    pub(crate) connect: Command,
    /// Present only when the installed app can update itself.
    pub(crate) check_for_updates: Option<Command>,
    pub(crate) reload: Command,
    pub(crate) enable_notifications: Command,
    /// Quits, stopping this computer's server first when the window shows
    /// it. Plain Quit leaves the server running for the other clients.
    pub(crate) quit_and_stop: Command,
}

fn item(label: &str, command: &Command) -> MenuItem {
    let command = command.clone();
    MenuItem::new(label).on_select(move || command())
}

impl Commands {
    pub(crate) fn tray(&self) -> Vec<MenuItem> {
        let mut items = vec![
            item("Show Aleph", &self.show),
            item("Open in Browser", &self.open_in_browser),
            item("Connect to Server…", &self.connect),
        ];
        if let Some(check) = &self.check_for_updates {
            items.push(item("Check for Updates…", check));
        }
        items.extend([
            item("Reload Panel", &self.reload),
            item("Enable Notifications", &self.enable_notifications),
            MenuItem::separator(),
            MenuItem::new("Quit Aleph").on_select(quit),
            item("Quit Aleph and Stop Server", &self.quit_and_stop),
        ]);
        items
    }

    /// The menu bar. Setting one replaces the platform's own, so it carries
    /// the standard app, Edit and Window items too; without Edit the Panel's
    /// text fields lose copy and paste.
    #[cfg(target_os = "macos")]
    pub(crate) fn menu_bar(&self) -> Vec<MenuItem> {
        let mut app = vec![MenuItem::role(MenuRole::About), MenuItem::separator()];
        if let Some(check) = &self.check_for_updates {
            app.extend([item("Check for Updates…", check), MenuItem::separator()]);
        }
        app.extend([
            MenuItem::role(MenuRole::Hide),
            MenuItem::role(MenuRole::HideOthers),
            MenuItem::role(MenuRole::ShowAll),
            MenuItem::separator(),
            item("Quit Aleph and Stop Server", &self.quit_and_stop),
            MenuItem::role(MenuRole::Quit),
        ]);
        let edit = [
            MenuItem::role(MenuRole::Undo),
            MenuItem::role(MenuRole::Redo),
            MenuItem::separator(),
            MenuItem::role(MenuRole::Cut),
            MenuItem::role(MenuRole::Copy),
            MenuItem::role(MenuRole::Paste),
            MenuItem::role(MenuRole::SelectAll),
        ];
        let view = [
            item("Reload Panel", &self.reload).shortcut("mod+r"),
            item("Open in Browser", &self.open_in_browser),
            MenuItem::separator(),
            item("Connect to Server…", &self.connect),
        ];
        // nsh makes the first submenu holding a window role the Window
        // menu macOS fills with its window items, so every window role
        // (full screen included) stays here.
        let window = [
            MenuItem::role(MenuRole::Minimize),
            MenuItem::role(MenuRole::Zoom),
            MenuItem::role(MenuRole::ToggleFullScreen),
            MenuItem::separator(),
            item("Show Aleph", &self.show),
            MenuItem::role(MenuRole::Close),
        ];
        vec![
            MenuItem::submenu("Aleph", app),
            MenuItem::submenu("Edit", edit),
            MenuItem::submenu("View", view),
            MenuItem::submenu("Window", window),
        ]
    }
}
