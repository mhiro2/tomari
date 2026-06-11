//! The menu-bar tray icon. Clicking it opens a native menu that surfaces a
//! permission setup affordance (when needed), quick window snaps and the
//! settings panel. The menu is rebuilt as permission state changes.

use tauri::menu::{CheckMenuItemBuilder, Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{App, AppHandle, Emitter, Manager};
use tomari_core::{AppAction, Language, WindowPreset};

use crate::actions;
use crate::state::AppState;

/// Stable id so the tray icon can be looked up again to toggle its visibility
/// or swap its menu.
const TRAY_ID: &str = "tomari-tray";

/// Every label in the tray menu, in one language.
struct Text {
    grant_accessibility: &'static str,
    grant_input: &'static str,
    window: &'static str,
    left_half: &'static str,
    right_half: &'static str,
    maximize: &'static str,
    center: &'static str,
    snap_window: &'static str,
    keep_awake: &'static str,
    open_settings: &'static str,
    check_updates: &'static str,
    quit: &'static str,
}

const TEXT_EN: Text = Text {
    grant_accessibility: "Grant Accessibility Access…",
    grant_input: "Grant Input Monitoring Access…",
    window: "Window",
    left_half: "Left Half",
    right_half: "Right Half",
    maximize: "Maximize",
    center: "Center",
    snap_window: "Snap Window",
    keep_awake: "Prevent Sleep",
    open_settings: "Open Settings…",
    check_updates: "Check for Updates",
    quit: "Quit Tomari",
};

const TEXT_JA: Text = Text {
    grant_accessibility: "アクセシビリティへのアクセスを許可…",
    grant_input: "入力監視へのアクセスを許可…",
    window: "ウィンドウ",
    left_half: "左半分",
    right_half: "右半分",
    maximize: "最大化",
    center: "中央",
    snap_window: "ウィンドウをスナップ",
    keep_awake: "スリープを防止",
    open_settings: "設定を開く…",
    check_updates: "アップデートを確認",
    quit: "Tomari を終了",
};

/// The menu text for the configured language, following the OS locale when the
/// setting is `System`.
fn text(app: &AppHandle) -> &'static Text {
    let language = app.state::<AppState>().settings.lock().unwrap().language;
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
        .icon(tauri::include_image!("icons/tray.png"))
        .icon_as_template(true)
        .tooltip("Tomari")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .build(app)?;

    Ok(())
}

/// Build the tray menu for the given permission state. Missing permissions get
/// an emphasized setup item at the very top; window snaps are disabled until
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

    let window_header = MenuItemBuilder::with_id("section:window", text.window)
        .enabled(false)
        .build(app)?;
    let snap_left = MenuItemBuilder::with_id("snap:leftHalf", text.left_half)
        .enabled(ax_granted)
        .build(app)?;
    let snap_right = MenuItemBuilder::with_id("snap:rightHalf", text.right_half)
        .enabled(ax_granted)
        .build(app)?;
    let snap_max = MenuItemBuilder::with_id("snap:maximize", text.maximize)
        .enabled(ax_granted)
        .build(app)?;
    let snap_center = MenuItemBuilder::with_id("snap:center", text.center)
        .enabled(ax_granted)
        .build(app)?;
    let snap = SubmenuBuilder::new(app, text.snap_window)
        .items(&[&snap_left, &snap_right, &snap_max, &snap_center])
        .build()?;

    // A checkmark reflects the live keep-awake state; clicking toggles it.
    let keep_awake_active = crate::keepawake::status(app.state::<AppState>().inner()).active;
    let keep_awake = CheckMenuItemBuilder::with_id("keep-awake", text.keep_awake)
        .checked(keep_awake_active)
        .build(app)?;

    let open = MenuItemBuilder::with_id("open", text.open_settings).build(app)?;
    let check_update = MenuItemBuilder::with_id("check-update", text.check_updates).build(app)?;
    let quit = MenuItemBuilder::with_id("quit", text.quit).build(app)?;

    menu.item(&window_header)
        .item(&snap)
        .separator()
        .item(&keep_awake)
        .separator()
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
/// global shortcut while the icon is hidden.
pub fn set_visible(app: &AppHandle, visible: bool) {
    if let Some(tray) = app.tray_by_id(TRAY_ID)
        && let Err(e) = tray.set_visible(visible)
    {
        tracing::warn!(error = %e, "failed to toggle tray visibility");
    }
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "setup:accessibility" => {
            request_accessibility();
            refresh(app);
            return;
        }
        "setup:input" => {
            request_input_monitoring();
            refresh(app);
            return;
        }
        "open" => {
            let _ = actions::show_panel(app);
            return;
        }
        "check-update" => {
            let _ = actions::show_panel(app);
            let _ = app.emit("tomari:check-update", ());
            return;
        }
        "keep-awake" => {
            // `toggle` rebuilds the menu (so the checkmark reflects the new
            // state) and emits the change event for the panel.
            crate::keepawake::toggle(app);
            return;
        }
        "quit" => {
            app.exit(0);
            return;
        }
        _ => {}
    }

    if let Some(rest) = id.strip_prefix("snap:")
        && let (Some(preset), Some(state)) = (preset_from_id(rest), app.try_state::<AppState>())
    {
        let _ = actions::dispatch(&AppAction::SnapWindow(preset), app, state.inner());
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

fn preset_from_id(id: &str) -> Option<WindowPreset> {
    Some(match id {
        "leftHalf" => WindowPreset::LeftHalf,
        "rightHalf" => WindowPreset::RightHalf,
        "maximize" => WindowPreset::Maximize,
        "center" => WindowPreset::Center,
        _ => return None,
    })
}
