// Tomari — a macOS menu-bar app for keyboard customization and window snapping.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod capsmap;
mod commands;
#[cfg(target_os = "macos")]
mod displays;
#[cfg(target_os = "macos")]
mod drag_to_move;
#[cfg(target_os = "macos")]
mod drag_to_snap;
mod error;
#[cfg(target_os = "macos")]
mod eventtap;
mod keepawake;
#[cfg(target_os = "macos")]
mod keycodes;
#[cfg(target_os = "macos")]
mod keysend;
mod locks;
#[cfg(target_os = "macos")]
mod overlay;
mod shortcuts;
mod state;
#[cfg(target_os = "macos")]
mod tap;
mod tray;
mod validate;
#[cfg(target_os = "macos")]
mod wake;
mod window_ops;

use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::ShortcutState;
use tomari_core::{AppPaths, AppSettings, Database, defaults};
use tomari_keyboard::ModifierEngine;
use tomari_window::WindowManager;

use crate::locks::MutexExt;
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
        // instance — two event taps would double-fire every remap and tap
        // action. The callback surfaces the existing instance's panel.
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
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    let Some(state) = app.try_state::<AppState>() else {
                        return;
                    };
                    let entry = state.shortcuts.lock_safe().get(shortcut).cloned();
                    let Some(action) = entry else {
                        return;
                    };

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
            commands::input_monitoring_status,
            commands::request_input_monitoring,
            commands::set_hotkeys_suspended,
            commands::validate_accelerator,
            commands::run_action,
            commands::check_for_update,
            commands::install_update,
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
                let s = state.settings.lock_safe();
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

            // Start the drag-to-snap and drag-to-move taps when enabled.
            #[cfg(target_os = "macos")]
            drag_to_snap::restart(&handle);
            #[cfg(target_os = "macos")]
            drag_to_move::restart(&handle);

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
            // cheap status syscalls run each tick; the menu rebuild — the
            // event-tap restart — and the `tomari:permissions-changed` emit
            // for the frontend all happen on the main thread solely on a
            // change.
            #[cfg(target_os = "macos")]
            {
                let poll_handle = handle.clone();
                // Sample the state setup already observed (accessibility_status
                // and the tray were built above) so the first tick compares
                // against reality instead of `None` — otherwise a permission
                // granted within the first poll interval would read as "always
                // was granted" rather than a transition, and the dead taps would
                // never be revived.
                let initial = tray::permission_state(&poll_handle);
                std::thread::spawn(move || {
                    // Poll responsively while a permission is still missing, then
                    // ease off to a slow heartbeat once both are granted and
                    // stable — there is nothing left to react to but the rare
                    // revocation, so a 2 s spin would be pure waste.
                    const FAST: std::time::Duration = std::time::Duration::from_secs(2);
                    const SLOW: std::time::Duration = std::time::Duration::from_secs(30);
                    let mut last = Some(initial);
                    let mut interval = if initial == (true, true) { SLOW } else { FAST };
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
                                drag_to_move::restart(&refresh_handle);
                            }
                            tray::refresh(&refresh_handle);
                            let _ = refresh_handle.emit(
                                "tomari:permissions-changed",
                                commands::PermissionsChanged {
                                    accessibility: current.0,
                                    input_monitoring: current.1,
                                },
                            );
                        });
                    }
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // The window hides on close rather than being destroyed, so the
            // close button just tucks it away and reopening is instant and
            // keeps its state. As a normal macOS window it stays open until
            // closed — it does not auto-hide when it loses focus.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        // Startup must not panic (see `build_state`'s doc comment): `.expect`
        // here would be exactly that, an invisible crash loop for a login-item
        // Accessory with no Dock icon or terminal. Route a build failure
        // through the same native-alert-and-exit path as every other
        // unrecoverable startup error instead.
        .unwrap_or_else(|e| fatal_startup_error(&format!("Tomari could not start: {e}")))
        // Release sleep prevention before the process exits — including the
        // lid-close override, which would otherwise outlive Tomari and keep the
        // Mac awake. This catches the tray Quit (`app.exit`) and a normal
        // quit/logout; the updater's `restart` does not guarantee this event,
        // so it calls `cleanup_blocking` itself. The write-ahead marker is the
        // backstop for any exit path that slips past both.
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // Restore Caps Lock's native behavior first: the HID remap
                // persists until reboot or removal, so a quit must take it
                // back down, and `hidutil` needs no permission and returns
                // quickly. Doing this *before* `cleanup_blocking` — which can
                // sit behind the admin-auth dialog for the lid-close veto —
                // means Caps Lock is never left remapped for however long that
                // dialog is up (or declined).
                let _ = capsmap::reconcile(false);
                keepawake::cleanup_blocking(app);
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
    // Window placement is gated behind the master switch, so an external
    // process cannot move the user's windows when they have opted out.
    // `toggle-panel` is exempt: it only shows/hides Tomari's own panel and is
    // the recovery route for a hidden menu bar, so it must keep working.
    if external.is_window_placement() && !state.settings.lock_safe().external_window_actions_enabled
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
/// the tap misbehaves. Key contents are never logged — this only adds a
/// destination.
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
    // settings row (plus an otherwise-empty database, checked below). The
    // settings row — not empty tables — is the primary marker so that a user who
    // deliberately clears all of their hotkeys or rules does not get them back.
    match db.settings_exist() {
        // Already initialized: leave the user's data alone.
        Ok(true) => {}
        // No settings row — a first run *if* the database is otherwise empty.
        // Guard against seeding over an inconsistent database that has hotkey or
        // rule rows but no settings row (an older build could write those before
        // a failed settings write): `seed_defaults` upserts by primary key, so
        // seeding would overwrite any user row whose id matches a default. Only
        // seed a truly pristine database; on a raw-count read failure, treat the
        // database as non-empty and skip, never risking a clobber.
        Ok(false) => {
            let has_rows =
                db.count_hotkeys().unwrap_or(1) > 0 || db.count_modifier_rules().unwrap_or(1) > 0;
            if has_rows {
                tracing::warn!(
                    "settings row missing but hotkeys or rules exist; skipping first-run seed to avoid overwriting existing data"
                );
            } else if let Err(e) = db.seed_defaults(
                &defaults::default_hotkeys(),
                &defaults::default_modifier_rules(),
                &AppSettings::default(),
            ) {
                tracing::error!(error = %e, "could not seed first-run defaults");
                alert(
                    "Tomari could not save its initial settings. It is running with \
                     built-in defaults for now; they will be stored on your next change.",
                    false,
                );
            }
        }
        // A read failure is *not* a first run: the settings row may well exist
        // but be momentarily unreadable (a lock, a transient SQLite error).
        // Seeding now would overwrite a real user's configuration, so touch
        // nothing on disk and run this session on the fallbacks the reads below
        // already provide (each surfaces its own alert if it, too, fails).
        Err(e) => {
            tracing::error!(error = %e, "could not determine first-run state; leaving the database untouched");
        }
    }

    // A read failure here is a row that opened fine but no longer decodes (a
    // corrupt JSON blob, or a value a newer build wrote) — distinct from the
    // unreadable *file* `open_database` already handled. Falling back keeps the
    // app running, but never silently: the loss is logged and surfaced so the
    // user knows their saved values are not in effect, rather than discovering it
    // only when a later save overwrites them.
    let settings = db.get_settings().unwrap_or_else(|e| {
        tracing::error!(error = %e, "could not read saved settings; using defaults for this session");
        alert(
            "Tomari could not read your saved settings, so it is running with defaults \
             for now. Your settings file was left in place.",
            false,
        );
        AppSettings::default()
    });

    // The stored modifier rules plus the built-in left/right ⌘ IME toggle,
    // which lives behind a setting rather than as an editable row.
    let loaded_rules = db.list_modifier_rules();
    // `Some(n)` only when the list actually read, so the per-row drop check below
    // is skipped on a hard read failure (already surfaced by the fallback).
    let decoded_rule_count = loaded_rules.as_ref().ok().map(Vec::len);
    let mut rules = loaded_rules.unwrap_or_else(|e| {
        tracing::error!(error = %e, "could not read saved keyboard rules; starting with none for this session");
        alert(
            "Tomari could not read your saved keyboard rules, so none are active for \
             now. Your saved rules were left in place.",
            false,
        );
        Vec::new()
    });

    // A *whole-list* read failure is surfaced above; an individual row whose
    // stored JSON no longer decodes is instead skipped silently by the list
    // queries (one bad row must not lose the whole list). Compare the raw row
    // counts with what decoded so that silent loss is surfaced too.
    warn_on_undecodable_rows(&db, decoded_rule_count);

    if settings.command_ime_switch_enabled {
        rules.extend(defaults::command_ime_rules());
    }
    let engine = ModifierEngine::new(rules);

    AppState::new(db, engine, make_window_manager(), settings)
}

/// Alert (once) when the database holds hotkey or rule rows that no longer
/// decode — which the list queries skip silently — so a vanished shortcut or
/// rule is visible rather than a mystery. `decoded_rules` is the rule count
/// already read in [`build_state`] (reused to avoid a second query); `None` when
/// that read failed, in which case the rule drop check is skipped because the
/// failure was already surfaced.
fn build_drop_count(decoded: Option<usize>, total: Result<usize, tomari_core::Error>) -> usize {
    match (decoded, total) {
        (Some(decoded), Ok(total)) => total.saturating_sub(decoded),
        _ => 0,
    }
}

fn warn_on_undecodable_rows(db: &Database, decoded_rules: Option<usize>) {
    let rules_dropped = build_drop_count(decoded_rules, db.count_modifier_rules());
    // Hotkeys are not otherwise loaded here, so read both counts for the check;
    // only a successful list paired with a successful count flags a real drop.
    let hotkeys_dropped =
        build_drop_count(db.list_hotkeys().ok().map(|h| h.len()), db.count_hotkeys());
    if rules_dropped == 0 && hotkeys_dropped == 0 {
        return;
    }
    tracing::error!(
        rules = rules_dropped,
        hotkeys = hotkeys_dropped,
        "skipping saved rows that no longer decode"
    );
    alert(
        "Some of your saved keyboard rules or shortcuts could not be read and were \
         skipped. The rest are unaffected, and nothing was deleted.",
        false,
    );
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
        // The message text is not under our control end-to-end (it can carry a
        // DB error's `Display` text), so it must not be interpolated into the
        // AppleScript source itself — Rust's `{:?}` Debug escaping is not
        // AppleScript string-literal escaping and is not a safety boundary.
        // Instead the script reads it from `argv`: everything after `--` is
        // passed through to the script unmodified as its `argv`, so
        // `item 1 of argv` is always exactly this string, whatever it
        // contains — no quoting/escaping step for it to defeat. Uses
        // `/usr/bin/osascript` (an absolute path), matching every other
        // `osascript` call in the app (see `keepawake.rs`).
        let script =
            "on run argv\n  display alert \"Tomari\" message (item 1 of argv) as critical\nend run";
        let mut cmd = std::process::Command::new("/usr/bin/osascript");
        cmd.arg("-e").arg(script).arg("--").arg(message);
        if blocking {
            let _ = cmd.status();
        } else {
            // Fire-and-forget from the caller's point of view, but the child
            // must still be `wait`ed eventually or it lingers as a zombie
            // until Tomari exits. Reap it on a worker thread so `alert` itself
            // stays non-blocking.
            match cmd.spawn() {
                Ok(mut child) => {
                    let _ = std::thread::Builder::new()
                        .name("tomari-alert-reap".into())
                        .spawn(move || {
                            let _ = child.wait();
                        });
                }
                Err(e) => tracing::warn!(error = %e, "failed to spawn osascript for alert"),
            }
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
