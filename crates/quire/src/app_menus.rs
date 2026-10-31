//! The native application menu, built from actions the way Zed's `app_menus` is, and the window
//! commands it offers. Every item dispatches an action, so the menu, the keyboard, and Help's
//! shortcut list stay one set of commands. Checked items follow the appearance setting and the
//! active profile.

use crate::{
    help::OpenHelp,
    navigation::{FindPage, NewPage, OpenToday, ToggleSidePane},
    preferences::{
        OpenPreferences, Preferences, UseDarkAppearance, UseLightAppearance, UseSystemAppearance,
    },
    profiles::{
        OpenNotebookDatabase, OpenNotebookFolder, UseCurrentProfile, UsePersonalProfile,
        UseWorkProfile,
    },
    showcase::OpenShowcase,
};
use gpui::{App, KeyBinding, Menu, MenuItem, OsAction};
use quire_store::profiles::ProfileId;
use settings::ThemeAppearanceMode;

gpui::actions!(
    quire_window,
    [
        /// Minimizes the active window.
        Minimize,
        /// Fills the screen with the active window, or returns it to its size.
        Zoom,
        /// Enters or leaves full screen.
        ToggleFullScreen,
        /// Closes the active window once its pages are recorded.
        CloseWindow,
        /// Hides Quire.
        HideQuire,
        /// Hides other applications.
        HideOthers,
        /// Shows every application again.
        ShowAll,
        /// Makes Quire's text and controls larger, until Quire quits or Actual Size.
        ZoomIn,
        /// Makes Quire's text and controls smaller, until Quire quits or Actual Size.
        ZoomOut,
        /// Returns text and controls to the sizes Settings choose.
        ActualSize,
        /// Shows or hides the sidebar.
        ToggleSidebar,
        /// Shows or hides the page details: its headings and connections.
        TogglePageDetails,
        /// Shows the notebook's folder in Finder or Explorer.
        ShowNotebookFolder,
        /// Turns Vim bindings on or off for every editor.
        ToggleVim,
    ]
);

pub fn init(cx: &mut App) {
    let on_active = |act: fn(&mut gpui::Window)| {
        move |cx: &mut App| {
            if let Some(window) = cx.active_window() {
                window.update(cx, |_, window, _| act(window)).ok();
            }
        }
    };
    let minimize = on_active(|window| window.minimize_window());
    let zoom = on_active(|window| window.zoom_window());
    let full_screen = on_active(|window| window.toggle_fullscreen());
    let close = on_active(|window| window.remove_window());
    cx.on_action(move |_: &Minimize, cx| minimize(cx));
    cx.on_action(move |_: &Zoom, cx| zoom(cx));
    cx.on_action(move |_: &ToggleFullScreen, cx| full_screen(cx));
    cx.on_action(move |_: &CloseWindow, cx| close(cx));
    cx.on_action(|_: &HideQuire, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &ZoomIn, cx| {
        theme_settings::increase_buffer_font_size(cx);
        theme_settings::adjust_ui_font_size(cx, |size| size + gpui::px(1.));
    });
    cx.on_action(|_: &ZoomOut, cx| {
        theme_settings::decrease_buffer_font_size(cx);
        theme_settings::adjust_ui_font_size(cx, |size| size - gpui::px(1.));
    });
    cx.on_action(|_: &ActualSize, cx| {
        theme_settings::reset_buffer_font_size(cx);
        theme_settings::reset_ui_font_size(cx);
    });
    cx.on_action(|_: &ToggleVim, cx| {
        let vim = crate::preferences::Preferences::current(cx).vim;
        crate::preferences::change(crate::preferences::Change::Vim(!vim), cx);
    });
    cx.observe_global::<settings::SettingsStore>(refresh)
        .detach();
    refresh(cx);
}

/// Sizes `window`'s interface from the UI font size, which View zoom changes, so every window
/// zooms with the notebook windows. Each window's render calls it.
pub fn follow_zoom(window: &mut gpui::Window, cx: &App) {
    window.set_rem_size(
        <theme_settings::ThemeSettings as settings::Settings>::get_global(cx).ui_font_size(cx),
    );
}

pub fn bind_keys(cx: &mut App) {
    let [zoom_in, zoom_in_plus, zoom_out, actual] = if cfg!(target_os = "macos") {
        ["cmd-=", "cmd-+", "cmd--", "cmd-0"]
    } else {
        ["ctrl-=", "ctrl-+", "ctrl--", "ctrl-0"]
    };
    cx.bind_keys([
        KeyBinding::new(zoom_in, ZoomIn, None),
        KeyBinding::new(zoom_in_plus, ZoomIn, None),
        KeyBinding::new(zoom_out, ZoomOut, None),
        KeyBinding::new(actual, ActualSize, None),
    ]);
    if cfg!(target_os = "macos") {
        cx.bind_keys([
            KeyBinding::new("cmd-m", Minimize, None),
            KeyBinding::new("cmd-shift-w", CloseWindow, None),
            KeyBinding::new("cmd-h", HideQuire, None),
            KeyBinding::new("cmd-alt-h", HideOthers, None),
            KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
        ]);
    } else {
        cx.bind_keys([
            KeyBinding::new("ctrl-shift-w", CloseWindow, None),
            KeyBinding::new("f11", ToggleFullScreen, None),
        ]);
    }
}

/// Sets the menu again, so checked items show the current appearance and profile.
pub fn refresh(cx: &mut App) {
    let menus = menus(cx);
    cx.set_menus(menus);
}

pub fn menus(cx: &App) -> Vec<Menu> {
    let appearance = cx
        .has_global::<settings::SettingsStore>()
        .then(|| Preferences::current(cx).appearance);
    let vim = cx.has_global::<settings::SettingsStore>() && Preferences::current(cx).vim;
    let profile = cx
        .has_global::<crate::profiles::ProfileStore>()
        .then(|| crate::profiles::active(cx));
    let profile_item =
        |name: &'static str, action: Box<dyn gpui::Action>, id: ProfileId| MenuItem::Action {
            name: name.into(),
            action,
            os_action: None,
            checked: profile == Some(id),
            disabled: false,
        };
    let appearance_item =
        |name: &'static str, action: Box<dyn gpui::Action>, mode: ThemeAppearanceMode| {
            MenuItem::Action {
                name: name.into(),
                action,
                os_action: None,
                checked: appearance == Some(mode),
                disabled: false,
            }
        };
    let mut menus = Vec::new();
    if cfg!(target_os = "macos") {
        menus.push(Menu::new("Quire").items([
            MenuItem::action("Settings…", OpenPreferences),
            MenuItem::separator(),
            MenuItem::action("Hide Quire", HideQuire),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Quire", crate::Quit),
        ]));
    }
    let mut file = vec![
        MenuItem::action("New Page…", NewPage),
        MenuItem::action("Today", OpenToday),
        MenuItem::separator(),
        MenuItem::action("Open Notebook Folder…", OpenNotebookFolder),
        MenuItem::action("Open Quire Notebook…", OpenNotebookDatabase),
        MenuItem::submenu(Menu::new("Profile").items([
            profile_item("Current", Box::new(UseCurrentProfile), ProfileId::Current),
            profile_item("Work", Box::new(UseWorkProfile), ProfileId::Work),
            profile_item(
                "Personal",
                Box::new(UsePersonalProfile),
                ProfileId::Personal,
            ),
        ])),
        MenuItem::separator(),
        MenuItem::action("Save Locally", crate::editor_command::SavePage),
        MenuItem::separator(),
        MenuItem::submenu(Menu::new("Import").items([
            MenuItem::action("Markdown Folder…", crate::exchange::ImportMarkdownFolder),
            MenuItem::action("Roam Data…", crate::exchange::ImportRoamData),
        ])),
        MenuItem::submenu(Menu::new("Export").items([
            MenuItem::action("Page as Markdown…", crate::exchange::ExportPageMarkdown),
            MenuItem::action(
                "Notebook as Markdown…",
                crate::exchange::ExportNotebookMarkdown,
            ),
            MenuItem::action("Notebook as Roam JSON…", crate::exchange::ExportRoamJson),
        ])),
        MenuItem::separator(),
        MenuItem::action("Backups…", crate::backups::ShowBackups),
        MenuItem::action("Back Up Now", crate::backups::BackUpNow),
        MenuItem::action("Sync with Git", crate::sync_status::SyncWithGit),
        MenuItem::action("Move from rt…", crate::rt_migration::view::OpenMigration),
        MenuItem::action("Show Notebook Folder", ShowNotebookFolder),
        MenuItem::separator(),
        MenuItem::action("Close Window", CloseWindow),
    ];
    if !cfg!(target_os = "macos") {
        file.extend([
            MenuItem::action("Settings…", OpenPreferences),
            MenuItem::separator(),
            MenuItem::action("Quit", crate::Quit),
        ]);
    }
    menus.push(Menu::new("File").items(file));
    menus.push(Menu::new("Edit").items([
        MenuItem::os_action("Undo", editor::actions::Undo, OsAction::Undo),
        MenuItem::os_action("Redo", editor::actions::Redo, OsAction::Redo),
        MenuItem::separator(),
        MenuItem::os_action("Cut", editor::actions::Cut, OsAction::Cut),
        MenuItem::os_action("Copy", editor::actions::Copy, OsAction::Copy),
        MenuItem::os_action("Paste", editor::actions::Paste, OsAction::Paste),
        MenuItem::os_action(
            "Select All",
            editor::actions::SelectAll,
            OsAction::SelectAll,
        ),
        MenuItem::separator(),
        MenuItem::submenu(Menu::new("Find").items([
            MenuItem::action("Find…", search::buffer_search::Deploy::find()),
            MenuItem::action(
                "Find and Replace…",
                search::buffer_search::Deploy::replace(),
            ),
            MenuItem::action("Find Next", search::SelectNextMatch),
            MenuItem::action("Find Previous", search::SelectPreviousMatch),
            MenuItem::separator(),
            MenuItem::action("Find in Notebook…", FindPage),
        ])),
    ]));
    menus.push(Menu::new("View").items([
        MenuItem::submenu(Menu::new("Appearance").items([
            appearance_item(
                "System",
                Box::new(UseSystemAppearance),
                ThemeAppearanceMode::System,
            ),
            appearance_item(
                "Light",
                Box::new(UseLightAppearance),
                ThemeAppearanceMode::Light,
            ),
            appearance_item(
                "Dark",
                Box::new(UseDarkAppearance),
                ThemeAppearanceMode::Dark,
            ),
        ])),
        MenuItem::separator(),
        MenuItem::action("Source or Write", crate::editor_host::TogglePresentation),
        MenuItem::action("Page Beside", ToggleSidePane),
        MenuItem::action("Sidebar", ToggleSidebar),
        MenuItem::action("Page Details", TogglePageDetails),
        MenuItem::Action {
            name: "Vim Bindings".into(),
            action: Box::new(ToggleVim),
            os_action: None,
            checked: vim,
            disabled: false,
        },
        MenuItem::separator(),
        MenuItem::action("Actual Size", ActualSize),
        MenuItem::action("Zoom In", ZoomIn),
        MenuItem::action("Zoom Out", ZoomOut),
        MenuItem::separator(),
        MenuItem::action("Enter Full Screen", ToggleFullScreen),
    ]));
    menus.push(Menu::new("Window").items([
        MenuItem::action("Minimize", Minimize),
        MenuItem::action("Zoom", Zoom),
    ]));
    menus.push(Menu::new("Help").items([
        MenuItem::action("Help and Shortcuts", OpenHelp),
        MenuItem::action("README Sample", OpenShowcase),
    ]));
    menus
}
