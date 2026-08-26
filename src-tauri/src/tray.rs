//! The menu-bar tray icon. Clicking it opens a native menu that surfaces a
//! permission setup affordance (when needed), reversible recovery actions, and
//! a Settings entry that opens the window. Direct placement stays on keyboard
//! and drag workflows instead of turning the tray into a window palette.

use tauri::menu::{CheckMenuItemBuilder, Menu, MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{App, AppHandle, Emitter, Manager};
use tomari_core::{AppAction, Language};

use crate::actions;
use crate::locks::MutexExt;
use crate::state::AppState;

/// Stable id so the tray icon can be looked up again to toggle its visibility
/// or swap its menu.
const TRAY_ID: &str = "tomari-tray";

/// Every label in the tray menu, in one language.
struct Text {
    grant_accessibility: &'static str,
    grant_input: &'static str,
    undo: &'static str,
    redo: &'static str,
    keep_awake: &'static str,
    expand_menu_bar: &'static str,
    open_settings: &'static str,
    check_updates: &'static str,
    quit: &'static str,
}

const TEXT_EN: Text = Text {
    grant_accessibility: "Grant Accessibility Access…",
    grant_input: "Grant Input Monitoring Access…",
    undo: "Undo Window Change",
    redo: "Redo Window Change",
    keep_awake: "Prevent Sleep",
    expand_menu_bar: "Show Menu Bar Icons",
    open_settings: "Settings…",
    check_updates: "Check for Updates",
    quit: "Quit Tomari",
};

const TEXT_JA: Text = Text {
    grant_accessibility: "アクセシビリティへのアクセスを許可…",
    grant_input: "入力監視へのアクセスを許可…",
    undo: "ウィンドウ操作を元に戻す",
    redo: "ウィンドウ操作をやり直す",
    keep_awake: "スリープ防止",
    expand_menu_bar: "メニューバーのアイコンを表示",
    open_settings: "設定…",
    check_updates: "アップデートを確認",
    quit: "Tomari を終了",
};

/// The menu text for the configured language, following the OS locale when the
/// setting is `System`.
fn text(app: &AppHandle) -> &'static Text {
    let language = app.state::<AppState>().settings.lock_safe().language;
    let japanese = match language {
        Language::Ja => true,
        Language::En => false,
        Language::System => system_is_japanese(),
    };
    if japanese { &TEXT_JA } else { &TEXT_EN }
}

fn system_is_japanese() -> bool {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::NSLocale;
        NSLocale::preferredLanguages()
            .iter()
            .next()
            .is_some_and(|lang| lang.to_string().starts_with("ja"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

pub fn build(app: &App) -> tauri::Result<()> {
    let (ax, im) = permission_state(app.handle());
    let menu = build_menu(app.handle(), ax, im)?;

    TrayIconBuilder::with_id(TRAY_ID)
        // macOS draws the tray image at 18pt whatever its pixel size, so embed
        // the 2x bitmap to stay crisp on Retina. Both sizes come from tray.svg.
        .icon(tauri::include_image!("icons/tray@2x.png"))
        .icon_as_template(true)
        .tooltip("Tomari")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .build(app)?;

    Ok(())
}

/// Build the tray menu for the given permission state. Missing permissions get
/// an emphasized setup item at the very top; recovery is disabled until
/// Accessibility is granted.
fn build_menu(
    app: &AppHandle,
    ax_granted: bool,
    im_granted: bool,
) -> tauri::Result<Menu<tauri::Wry>> {
    let text = text(app);
    let mut menu = MenuBuilder::new(app);

    let needs_setup = !ax_granted || !im_granted;
    if !ax_granted {
        menu = menu.item(
            &MenuItemBuilder::with_id("setup:accessibility", text.grant_accessibility)
                .build(app)?,
        );
    }
    if !im_granted {
        menu = menu.item(&MenuItemBuilder::with_id("setup:input", text.grant_input).build(app)?);
    }
    if needs_setup {
        menu = menu.separator();
    }

    let state = app.state::<AppState>();
    let window_enabled = state.settings.lock_safe().window_management_enabled;
    let (can_undo, can_redo) = state.window_history_status();
    let undo = MenuItemBuilder::with_id("undo", text.undo)
        .enabled(ax_granted && window_enabled && can_undo)
        .build(app)?;
    let redo = MenuItemBuilder::with_id("redo", text.redo)
        .enabled(ax_granted && window_enabled && can_redo)
        .build(app)?;

    // A checkmark reflects the live keep-awake state; clicking toggles it.
    let keep_awake_status = crate::keepawake::status(state.inner());
    let keep_awake = CheckMenuItemBuilder::with_id("keep-awake", text.keep_awake)
        .checked(keep_awake_status.active)
        .enabled(!keep_awake_status.phase.is_pending())
        .build(app)?;

    // Only offered while menu bar tidying is on: with the feature off there is
    // nothing hidden to show, and an item that does nothing is worse than none.
    let menu_bar = crate::menubar::status(app.state::<AppState>().inner());
    let expand_menu_bar = menu_bar.enabled.then(|| {
        CheckMenuItemBuilder::with_id("menu-bar-expand", text.expand_menu_bar)
            .checked(!menu_bar.collapsed)
            .build(app)
    });

    let open = MenuItemBuilder::with_id("open", text.open_settings).build(app)?;
    let check_update = MenuItemBuilder::with_id("check-update", text.check_updates).build(app)?;
    let quit = MenuItemBuilder::with_id("quit", text.quit).build(app)?;

    let mut menu = menu.item(&undo).item(&redo).separator().item(&keep_awake);
    if let Some(item) = expand_menu_bar {
        menu = menu.item(&item?);
    }
    menu.separator()
        .item(&open)
        .item(&check_update)
        .separator()
        .item(&quit)
        .build()
}

/// Rebuild and install the tray menu so it reflects the current permission
/// state. Must run on the main thread (it touches menu/tray UI).
pub fn refresh(app: &AppHandle) {
    let (ax, im) = permission_state(app);
    match build_menu(app, ax, im) {
        Ok(menu) => {
            if let Some(tray) = app.tray_by_id(TRAY_ID)
                && let Err(e) = tray.set_menu(Some(menu))
            {
                tracing::warn!(error = %e, "failed to update tray menu");
            }
        }
        Err(e) => tracing::warn!(error = %e, "failed to build tray menu"),
    }
}

/// Current (accessibility, input-monitoring) permission state.
pub fn permission_state(app: &AppHandle) -> (bool, bool) {
    let ax = app.state::<AppState>().windows.permission_granted();
    (ax, input_monitoring_granted())
}

fn input_monitoring_granted() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::eventtap::input_monitoring_granted()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Show or hide the menu-bar tray icon to honor the "Show in menu bar" setting.
/// With the Accessory activation policy the panel is still reachable via the
/// global shortcut while the icon is hidden. Returns whether it applied; a
/// failure (or a missing tray) is logged and reported as `false`.
pub fn set_visible(app: &AppHandle, visible: bool) -> bool {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        tracing::warn!("tray icon not found while toggling visibility");
        return false;
    };
    match tray.set_visible(visible) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "failed to toggle tray visibility");
            false
        }
    }
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "setup:accessibility" => {
            request_accessibility();
            refresh(app);
        }
        "setup:input" => {
            request_input_monitoring();
            refresh(app);
        }
        "open" => {
            let _ = actions::show_panel(app);
        }
        "check-update" => {
            let _ = actions::show_panel(app);
            let _ = app.emit("tomari:check-update", ());
        }
        "keep-awake" => {
            // `toggle` rebuilds the menu (so the checkmark reflects the new
            // state) and emits the change event for the panel.
            crate::keepawake::toggle(app);
        }
        "menu-bar-expand" => {
            // Same contract as keep-awake above: the toggle republishes, which
            // rebuilds this menu and notifies the panel.
            crate::menubar::toggle(app);
        }
        "undo" => {
            if let Some(state) = app.try_state::<AppState>() {
                let _ = actions::dispatch(&AppAction::UndoWindow, app, state.inner());
            }
        }
        "redo" => {
            if let Some(state) = app.try_state::<AppState>() {
                let _ = actions::dispatch(&AppAction::RedoWindow, app, state.inner());
            }
        }
        "quit" => {
            app.exit(0);
        }
        _ => {}
    }
}

fn request_accessibility() {
    #[cfg(target_os = "macos")]
    {
        tomari_window::request_permission();
    }
}

fn request_input_monitoring() {
    #[cfg(target_os = "macos")]
    {
        crate::eventtap::request_input_monitoring();
    }
}
