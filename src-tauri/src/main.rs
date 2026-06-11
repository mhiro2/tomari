// Tomari — a macOS menu-bar app for keyboard customization and window snapping.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod capsmap;
mod commands;
#[cfg(target_os = "macos")]
mod displays;
#[cfg(target_os = "macos")]
mod drag_to_snap;
mod error;
#[cfg(target_os = "macos")]
mod eventtap;
#[cfg(target_os = "macos")]
mod frontmost;
mod import_export;
mod keepawake;
#[cfg(target_os = "macos")]
mod keycodes;
#[cfg(target_os = "macos")]
mod keysend;
#[cfg(target_os = "macos")]
mod overlay;
#[cfg(target_os = "macos")]
mod peek;
mod shortcuts;
mod state;
mod tray;
#[cfg(target_os = "macos")]
mod wake;
mod window_ops;

use tauri::Manager;
use tauri_plugin_global_shortcut::ShortcutState;
use tomari_core::{AppPaths, AppSettings, Database, defaults};
use tomari_keyboard::ModifierEngine;
use tomari_window::WindowManager;

use crate::state::AppState;

fn main() {
    // Resolve the data directory before logging so the log file can live
    // under it; a resolution failure falls back to stderr-only logging and
    // turns fatal (with a visible alert) right after.
    let paths = AppPaths::resolve().and_then(|p| {
        p.ensure()?;
        Ok(p)
    });
    init_logging(paths.as_ref().ok());

    let app_state = match &paths {
        Ok(paths) => build_state(paths),
        Err(e) => fatal_startup_error(&format!("Tomari could not prepare its data directory: {e}")),
    };

    tauri::Builder::default()
        // Register first: a second launch must hand off to the running
        // instance — two event taps would double-fire every remap, tap action
        // and peek. The callback surfaces the existing instance's panel.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // A bare second launch surfaces the panel. On macOS `tomari://`
            // URLs are delivered to deep-link's `on_open_url`, not here, so this
            // path only ever means "the user opened Tomari again".
            let _ = actions::show_panel(app);
        }))
        // Registered right after single-instance, as the deep-link plugin
        // requires, so the already-running instance receives `tomari://` URLs.
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        // Native file pickers for settings import/export, driven from Rust.
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    let Some(state) = app.try_state::<AppState>() else {
                        return;
                    };
                    let entry = state.shortcuts.lock().unwrap().get(shortcut).cloned();
                    let Some(action) = entry else {
                        return;
                    };

                    // A Quick Peek hotkey acts while held: summon on press,
                    // dismiss on release.
                    #[cfg(target_os = "macos")]
                    if let tomari_core::AppAction::LaunchApp(target) = &action
                        && target.quick_peek
                    {
                        let trigger = peek::Trigger::Hotkey(shortcut.id());
                        if event.state() == ShortcutState::Pressed {
                            peek::begin(app, trigger, &target.bundle_id);
                        } else {
                            peek::end(app, trigger);
                        }
                        return;
                    }

                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    if let Err(e) = actions::dispatch(&action, app, state.inner()) {
                        tracing::warn!(error = %e, "shortcut action failed");
                    }
                })
                .build(),
        )
        .manage(app_state)
        .manage(commands::PendingUpdate::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::list_hotkeys,
            commands::save_hotkey,
            commands::delete_hotkey,
            commands::list_modifier_rules,
            commands::save_modifier_rule,
            commands::delete_modifier_rule,
            commands::list_window_presets,
            commands::snap_window,
            commands::move_window_to_display,
            commands::undo_window,
            commands::accessibility_status,
            commands::request_accessibility,
            commands::set_hotkeys_suspended,
            commands::validate_accelerator,
            commands::run_action,
            commands::check_for_update,
            commands::install_update,
            commands::export_config,
            commands::import_config,
            commands::get_keep_awake,
            commands::set_keep_awake,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            tray::build(app)?;

            let handle = app.handle().clone();
            let state = app.state::<AppState>();

            // Wire the `tomari://` URL scheme. The cold-start URL (Tomari was
            // launched by the link) and warm-start URLs (it was already running)
            // both funnel through the same handler; URLs are never read from
            // argv. The scheme is fire-and-forget — a bad URL or a disabled
            // master switch is logged and dropped inside the handler.
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    for url in urls {
                        dispatch_deep_link(&handle, url.as_str());
                    }
                }
                let dl_handle = handle.clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        dispatch_deep_link(&dl_handle, url.as_str());
                    }
                });
            }

            // Apply the persisted menu-bar and login-item preferences on launch
            // so the actual system state matches what the settings show.
            let (show_tray, launch_at_login) = {
                let s = state.settings.lock().unwrap();
                (s.show_in_menu_bar, s.launch_at_login)
            };
            if !show_tray {
                tray::set_visible(&handle, false);
            }
            commands::apply_launch_at_login(&handle, launch_at_login);

            // Individual hotkeys that fail to register are logged (and
            // tolerated) inside `register_all`; only failing to read the
            // hotkey list at all lands here.
            if let Err(e) = shortcuts::register_all(&handle, state.inner()) {
                tracing::error!(error = %e, "failed to register global shortcuts");
            }

            // Start the keyboard event tap (Input Monitoring). Attempting this
            // even before the permission is granted adds Tomari to the Input
            // Monitoring list so the user can enable it.
            #[cfg(target_os = "macos")]
            eventtap::restart(&handle);

            // Prime the drag-to-snap display-geometry cache and keep it current
            // on display changes — before the drag-to-snap tap starts, so the
            // first drag always has geometry to snap against.
            #[cfg(target_os = "macos")]
            displays::install(&handle);

            // Start the drag-to-snap tap when the feature is enabled.
            #[cfg(target_os = "macos")]
            drag_to_snap::restart(&handle);

            // A sleep or session switch can swallow key releases; reset the
            // key-tracking state whenever the system comes back.
            #[cfg(target_os = "macos")]
            wake::install(&handle);

            // Keep-awake never persists as "on", so clear any lid-close sleep
            // override a previous run left behind after an unclean exit.
            keepawake::reconcile_on_launch(&handle);

            // Permissions are granted in System Settings, outside the app, so
            // poll their state and react on a transition (the native left-click
            // menu has no "about to open" hook to do this lazily). Only the
            // cheap status syscalls run each tick; the menu rebuild — and the
            // event-tap restart below — happen on the main thread solely on a
            // change.
            #[cfg(target_os = "macos")]
            {
                let poll_handle = handle.clone();
                std::thread::spawn(move || {
                    // Poll responsively while a permission is still missing, then
                    // ease off to a slow heartbeat once both are granted and
                    // stable — there is nothing left to react to but the rare
                    // revocation, so a 2 s spin would be pure waste.
                    const FAST: std::time::Duration = std::time::Duration::from_secs(2);
                    const SLOW: std::time::Duration = std::time::Duration::from_secs(30);
                    let mut last: Option<(bool, bool)> = None;
                    let mut interval = FAST;
                    loop {
                        std::thread::sleep(interval);
                        let current = tray::permission_state(&poll_handle);
                        if last == Some(current) {
                            interval = if current == (true, true) { SLOW } else { FAST };
                            continue;
                        }
                        // A change (including a revocation): return to responsive
                        // polling until things settle again.
                        interval = FAST;
                        // The event taps created at launch return a null tap
                        // when Input Monitoring is missing and stay dead until
                        // restarted, so revive them when it is newly granted.
                        let input_monitoring_granted =
                            matches!(last, Some((_, was_im)) if !was_im) && current.1;
                        last = Some(current);
                        let refresh_handle = poll_handle.clone();
                        let _ = poll_handle.run_on_main_thread(move || {
                            if input_monitoring_granted {
                                eventtap::restart(&refresh_handle);
                                drag_to_snap::restart(&refresh_handle);
                            }
                            tray::refresh(&refresh_handle);
                        });
                    }
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| match event {
            // The window is a popover: closing or losing focus just hides it.
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            tauri::WindowEvent::Focused(false) => {
                let _ = window.hide();
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("error while building Tomari")
        // Release sleep prevention before the process exits — including the
        // lid-close override, which would otherwise outlive Tomari and keep the
        // Mac awake. This catches the tray Quit (`app.exit`) and a normal
        // quit/logout; the updater's `restart` does not guarantee this event,
        // so it calls `cleanup_blocking` itself. The write-ahead marker is the
        // backstop for any exit path that slips past both.
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                keepawake::cleanup_blocking(app);
                // Restore Caps Lock's native behavior — the HID remap persists
                // until reboot or removal, so a quit must take it back down.
                let _ = capsmap::reconcile(false);
            }
        });
}

/// Resolve a `tomari://` URL to an action and run it. Fire-and-forget: the
/// launcher has already moved on, so there is no caller to return a result to —
/// a malformed URL, a disabled master switch, or a failed action is logged and
/// dropped rather than surfaced.
fn dispatch_deep_link(app: &tauri::AppHandle, raw: &str) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let external = match tomari_core::parse_deep_link(raw) {
        Ok(action) => action,
        Err(e) => {
            tracing::warn!(url = %raw, error = %e, "ignoring malformed tomari:// URL");
            return;
        }
    };
    // Window placement (toggle-panel included) is gated behind the canary
    // master switch, so an external process cannot drive Tomari when the user
    // has opted out.
    if !state
        .settings
        .lock()
        .unwrap()
        .external_window_actions_enabled
    {
        tracing::warn!(url = %raw, "external window actions disabled; ignoring tomari:// URL");
        return;
    }
    // dispatch does exactly what the action says — a snap never summons the
    // panel — so Tomari does not steal frontmost from the window being placed.
    let action: tomari_core::AppAction = external.into();
    if let Err(e) = actions::dispatch(&action, app, state.inner()) {
        tracing::warn!(url = %raw, error = %e, "tomari:// action failed");
    }
}

/// How many daily log files to keep before the oldest is pruned.
const LOG_KEEP_FILES: usize = 7;

/// Route logs to stderr and, when the data directory is known, to a
/// daily-rotated file under `<data_dir>/logs`. Launched as a login item the
/// app has no terminal, so without the file there is nowhere to look when
/// the tap or a peek misbehaves. Key contents are never logged — this only
/// adds a destination.
fn init_logging(paths: Option<&AppPaths>) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "tomari=info,warn".into());

    let file_layer = paths.and_then(|p| {
        match tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("tomari")
            .filename_suffix("log")
            .max_log_files(LOG_KEEP_FILES)
            .build(p.data_dir.join("logs"))
        {
            Ok(appender) => Some(
                tracing_subscriber::fmt::layer()
                    .with_writer(appender)
                    .with_ansi(false),
            ),
            Err(e) => {
                eprintln!("tomari: file logging disabled: {e}");
                None
            }
        }
    });

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(file_layer)
        .try_init();
}

/// Open the database, seed first-run defaults, and assemble shared state.
///
/// Startup must not panic: as an Accessory (no Dock icon, no window) launched
/// at login, a panic is a silent crash loop with no feedback at all. Anything
/// unrecoverable shows a native alert once and exits instead.
fn build_state(paths: &AppPaths) -> AppState {
    let db = open_database(paths);

    // Seed defaults only on the very first run, detected by the absence of the
    // settings row. Keying off empty tables would resurrect defaults whenever a
    // user deliberately clears all of their hotkeys or rules.
    if !db.settings_exist().unwrap_or(false) {
        for hk in defaults::default_hotkeys() {
            let _ = db.upsert_hotkey(&hk);
        }
        for rule in defaults::default_modifier_rules() {
            let _ = db.upsert_modifier_rule(&rule);
        }
        let _ = db.save_settings(&AppSettings::default());
    }

    // Sanitize on load too, not just on save: a pre-existing out-of-range
    // value or a hand-edited database would otherwise drive the engines (and
    // `get_settings`, which reads the row directly) until the next save.
    // Persist the correction only when it actually changed something.
    let mut settings = db.get_settings().unwrap_or_default();
    let raw = settings.clone();
    settings.sanitize();
    if settings != raw {
        let _ = db.save_settings(&settings);
    }
    let rules = db.list_modifier_rules().unwrap_or_default();
    let engine = ModifierEngine::new(rules, settings.hold_threshold_ms);

    AppState::new(db, engine, make_window_manager(), settings)
}

/// Open the SQLite database, surviving a damaged file: corruption is real
/// over years of running, and for a resident tool losing settings is less
/// fatal than never starting again. A *corrupt* database is moved aside
/// (kept for inspection) and a fresh one is created, with a one-time native
/// alert. Transient failures — a lock held by another process, a read-only
/// or full disk — must not discard a healthy database, so they exit with an
/// alert instead.
fn open_database(paths: &AppPaths) -> Database {
    let error = match Database::open(&paths.db_path) {
        Ok(db) => return db,
        Err(e) => e,
    };
    if error.is_database_corruption() {
        tracing::error!(error = %error, "database is corrupt — moving it aside and starting fresh");
        if move_database_aside(paths)
            && let Ok(db) = Database::open(&paths.db_path)
        {
            alert(
                "Tomari could not read its settings database, so it was reset. \
                 The unreadable file was kept next to it with a .broken suffix.",
                false,
            );
            return db;
        }
    }
    fatal_startup_error(&format!(
        "Tomari could not open its settings database: {error}"
    ));
}

/// Rename the database to `tomari.sqlite.broken-<unix-ms>` so a fresh one can
/// be created; returns whether the main file actually moved. The WAL/SHM
/// sidecars must not stay behind — SQLite would replay a stale WAL into the
/// fresh database — so one that cannot move is deleted.
fn move_database_aside(paths: &AppPaths) -> bool {
    let Some(name) = paths.db_path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let dst = paths.db_path.with_file_name(format!("{name}.broken-{ts}"));
    if let Err(e) = std::fs::rename(&paths.db_path, &dst) {
        tracing::error!(error = %e, "could not move the corrupt database aside");
        return false;
    }
    for suffix in ["-wal", "-shm"] {
        let src = paths.db_path.with_file_name(format!("{name}{suffix}"));
        if !src.exists() {
            continue;
        }
        let dst = paths
            .db_path
            .with_file_name(format!("{name}.broken-{ts}{suffix}"));
        if std::fs::rename(&src, &dst).is_err()
            && let Err(e) = std::fs::remove_file(&src)
        {
            tracing::warn!(error = %e, path = %src.display(), "could not move or delete a database sidecar");
        }
    }
    true
}

/// Show a native alert. The app may have no tray icon or window yet (or ever,
/// if startup fails), so this goes through `osascript` rather than the UI.
/// `blocking` waits for the dialog to be dismissed — used before exiting, so
/// the process does not vanish from under the message.
fn alert(message: &str, blocking: bool) {
    #[cfg(target_os = "macos")]
    {
        let script = format!("display alert \"Tomari\" message {message:?} as critical");
        let mut cmd = std::process::Command::new("osascript");
        cmd.arg("-e").arg(script);
        if blocking {
            let _ = cmd.status();
        } else {
            let _ = cmd.spawn();
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = blocking;
        eprintln!("{message}");
    }
}

/// Log and show an unrecoverable startup failure, then exit. Replaces a panic,
/// which for a background login item would be an invisible crash loop.
fn fatal_startup_error(message: &str) -> ! {
    tracing::error!("{message}");
    alert(message, true);
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn make_window_manager() -> Box<dyn WindowManager + Send + Sync> {
    Box::new(tomari_window::AxWindowManager::new())
}

#[cfg(not(target_os = "macos"))]
fn make_window_manager() -> Box<dyn WindowManager + Send + Sync> {
    Box::new(tomari_window::MockWindowManager::new(
        tomari_core::Rect::new(0.0, 0.0, 1440.0, 900.0),
    ))
}
