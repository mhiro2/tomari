// Tomari — a macOS menu-bar app for keyboard customization and window snapping.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod capsmap;
mod childproc;
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
mod instance_lock;
mod keepawake;
#[cfg(target_os = "macos")]
mod keycodes;
#[cfg(target_os = "macos")]
mod keysend;
mod locks;
mod menubar;
#[cfg(target_os = "macos")]
mod overlay;
mod regrant;
mod shortcuts;
mod state;
#[cfg(target_os = "macos")]
mod tap;
mod tray;
mod validate;
#[cfg(target_os = "macos")]
mod wake;
mod window_ops;

use std::path::Path;

use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::ShortcutState;
use tomari_core::{AppPaths, AppSettings, Database, defaults};
use tomari_keyboard::ModifierEngine;
use tomari_window::WindowManager;

use crate::instance_lock::{AcquireError, InstanceLock, Outcome};
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
    let paths = match paths {
        Ok(paths) => paths,
        Err(e) => fatal_startup_error(&format!("Tomari could not prepare its data directory: {e}")),
    };
    let context = tauri::generate_context!();

    // Before the database and before the Tauri builder: the process that holds
    // this lock is the instance. Everything below — the single-instance socket,
    // the database, the event taps — belongs to it alone. A launch that cannot
    // get the lock hands itself off to the holder over the plugin's own socket
    // and exits without ever registering the plugin, so the socket can never end
    // up owned by a process that does not also own the data directory (the
    // plugin would otherwise take an existing socket over when it binds).
    let identifier = context.config().identifier.clone();
    let lock = match InstanceLock::acquire_or_hand_off(&paths.data_dir, || {
        instance_lock::hand_off_to_holder(&identifier).is_ok()
    }) {
        Ok(Outcome::Locked(lock)) => lock,
        Ok(Outcome::HandedOff) => {
            tracing::info!("another instance holds the data directory; handed off");
            std::process::exit(0);
        }
        Err(AcquireError::Held) => {
            tracing::warn!("another instance holds the data directory but is not listening");
            fatal_startup_error(
                "Tomari is already running. If it is not, wait a moment and open it again.",
            );
        }
        Err(AcquireError::Io(e)) => fatal_startup_error(&format!(
            "Tomari could not lock its data directory {}: {e}",
            paths.data_dir.display()
        )),
    };
    let app_state = build_state(&paths);

    tauri::Builder::default()
        // Register first: a second launch must hand off to the running
        // instance — two event taps would double-fire every remap and tap
        // action. The callback surfaces the existing instance's panel. Only
        // the lock holder gets here, so only it ever binds the socket.
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
                    let Some(action) = shortcuts::action_for_shortcut(state.inner(), shortcut)
                    else {
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
        .manage(lock)
        .manage(app_state)
        .manage(commands::PendingUpdate::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::get_apply_warnings,
            commands::list_hotkeys,
            commands::save_hotkey,
            commands::delete_hotkey,
            commands::list_modifier_rules,
            commands::save_modifier_rule,
            commands::delete_modifier_rule,
            commands::undo_window,
            commands::redo_window,
            commands::get_window_history_status,
            commands::get_placement_context,
            commands::capture_window_placement,
            commands::forget_window_placement,
            commands::undo_window_placement_edit,
            commands::recall_window_placement,
            commands::move_window_to_display_and_recall,
            commands::setup_status,
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
            commands::configure_keep_awake,
            commands::cancel_keep_awake_transition,
            commands::retry_keep_awake_transition,
            commands::get_menu_bar,
            commands::list_menu_bar_items,
            commands::move_menu_bar_item,
            commands::set_menu_bar_collapsed,
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
            keepawake::start_monitor(&handle);

            // Put the menu bar divider back if tidying is switched on. Always
            // collapsed to start with, so a launch looks the same every time.
            menubar::init(&handle);

            // Compare the current permission state against the snapshot the
            // previous run stored: a grant that vanished together with a
            // version change means an update revoked it (ad-hoc signing does
            // this on every update), which the setup checklist should explain
            // proactively rather than letting the user discover taps that
            // silently stopped working. Same-version losses are the user's own
            // revocation and stay quiet. All of it is best-effort UX — a
            // snapshot that fails to read or write never affects startup.
            let initial = tray::permission_state(&handle);
            #[cfg(target_os = "macos")]
            drag_to_move::set_accessibility_granted(initial.0);
            #[cfg(target_os = "macos")]
            eventtap::set_accessibility_granted(initial.0);
            let app_version = app.package_info().version.to_string();
            {
                let prev = regrant::load_snapshot(&state.db);
                let update_regrant =
                    regrant::is_update_regrant(prev.as_ref(), initial, &app_version);
                state.set_update_regrant(update_regrant);
                regrant::store_snapshot(&state.db, initial, &app_version);
                if update_regrant {
                    let _ = actions::show_panel(&handle);
                }
            }

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
                // `initial` was sampled above (the tray was built from it too),
                // so the first tick compares against reality instead of `None` —
                // otherwise a permission granted within the first poll interval
                // would read as "always was granted" rather than a transition,
                // and the dead taps would never be revived.
                let poll_version = app_version.clone();
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
                        // The drag-to-move tap reads the Accessibility grant off
                        // an atomic rather than calling into TCC from its
                        // callback (which holds up all input), so this poll is
                        // what keeps that mirror current.
                        drag_to_move::set_accessibility_granted(current.0);
                        eventtap::set_accessibility_granted(current.0);
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
                        let refresh_version = poll_version.clone();
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
                            // Keep the stored snapshot tracking every observed
                            // transition, so the next launch compares against
                            // the state this run actually ended with.
                            if let Some(state) = refresh_handle.try_state::<AppState>() {
                                regrant::store_snapshot(&state.db, current, &refresh_version);
                            }
                        });
                    }
                });
            }

            // A true first run (the database was just seeded) auto-opens the
            // settings window once: launched as an Accessory there is no
            // window, no Dock icon, and every headline feature still waits on
            // permissions — without this, "nothing happened" is the whole
            // first impression. Later launches stay quiet as before. Safe
            // even before the WebView has finished loading; this only shows
            // the window.
            if state.first_run {
                let _ = actions::show_panel(&handle);
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
        .build(context)
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
                // Stop the keyboard tap now, releasing any remapped modifier it
                // still holds downstream — left running into the slow cleanup
                // below it would keep stamping stale targets, and dying with
                // the process it would leave the app holding them. This goes
                // *before* the Caps Lock restore: a Caps reconcile the tap had
                // put off for a hold runs under the same lock `teardown` takes,
                // so once it returns no such worker can re-enable the remap
                // behind the restore (one still to run sees no tap and turns it
                // off too).
                #[cfg(target_os = "macos")]
                eventtap::teardown(app);
                // Restore Caps Lock's native behavior: the HID remap persists
                // until reboot or removal, so a quit must take it back down,
                // and `hidutil` needs no permission and returns quickly. Doing
                // this *before* `cleanup_blocking` — which can sit behind the
                // admin-auth dialog for the lid-close veto — means Caps Lock is
                // never left remapped for however long that dialog is up (or
                // declined).
                // The outcome is logged, not dropped: a failed restore leaves
                // the claim record on disk, so the next launch's reconcile
                // retries it and the settings panel shows the mismatch until it
                // heals (`get_apply_warnings`).
                let outcome = capsmap::reconcile(false);
                if !outcome.reconciled {
                    tracing::warn!(
                        proxy_active = outcome.proxy_active,
                        "caps-lock HID remap could not be restored on quit; will retry at next launch"
                    );
                }
                // Drop the divider before the slow part below: it is the one
                // piece of teardown the user can see, and `cleanup_blocking`
                // can sit behind an admin-auth dialog for a while.
                menubar::teardown(app);
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
    let first_run = seed_first_run_defaults(&db);

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

    AppState::new(db, engine, make_window_manager(), settings, first_run)
}

/// Seed defaults only on the very first run, detected by the absence of the
/// settings row (plus an otherwise-empty database, checked below). The
/// settings row — not empty tables — is the primary marker so that a user who
/// deliberately clears all of their hotkeys or rules does not get them back.
///
/// Returns whether this launch is a true first run — the seed actually ran —
/// which `setup` uses to auto-open the settings window once. Every ambiguous
/// case (an unreadable row, an inconsistent database, a failed seed) returns
/// `false`: surprising an existing user with a window is worse than staying
/// quiet on a genuinely fresh install.
fn seed_first_run_defaults(db: &Database) -> bool {
    match db.settings_exist() {
        // Already initialized: leave the user's data alone.
        Ok(true) => false,
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
                return false;
            }
            if let Err(e) = db.seed_defaults(
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
                return false;
            }
            true
        }
        // A read failure is *not* a first run: the settings row may well exist
        // but be momentarily unreadable (a lock, a transient SQLite error).
        // Seeding now would overwrite a real user's configuration, so touch
        // nothing on disk and run this session on the fallbacks the reads in
        // `build_state` already provide (each surfaces its own alert if it,
        // too, fails).
        Err(e) => {
            tracing::error!(error = %e, "could not determine first-run state; leaving the database untouched");
            false
        }
    }
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
    if !sweep_orphan_sidecars(&RealFileOps, &paths.db_path, unix_ms()) {
        fatal_startup_error(&format!(
            "Tomari found -wal or -shm files left beside {} by an interrupted settings \
             reset, and could not move them aside. Move them somewhere else, then open \
             Tomari again.",
            paths.db_path.display()
        ));
    }
    let error = match Database::open(&paths.db_path) {
        Ok(db) => return db,
        Err(e) => e,
    };
    if error.is_database_corruption() {
        tracing::error!(error = %error, "database is corrupt — moving it aside and starting fresh");
        if !move_database_aside(paths) {
            // The corrupt set is still in place — either the database, or a
            // sidecar SQLite would replay into whatever replaced it. A "fresh"
            // database beside those would not be fresh, so name the files to
            // deal with rather than start on top of them.
            fatal_startup_error(&format!(
                "Tomari's settings database is damaged, and could not be moved aside \
                 automatically. Move {} — along with any -wal or -shm file beside it \
                 — somewhere else, then open Tomari again.",
                paths.db_path.display()
            ));
        }
        match Database::open(&paths.db_path) {
            Ok(db) => {
                alert(
                    "Tomari could not read its settings database, so it was reset. \
                     The unreadable file was kept next to it with a .broken suffix.",
                    false,
                );
                return db;
            }
            // The damaged database is aside by now, so the corruption error that
            // got us here explains nothing about *this* failure — report the one
            // that actually stopped us.
            Err(fresh) => fatal_startup_error(&format!(
                "Tomari moved its damaged settings database aside but could not create \
                 a new one: {fresh}"
            )),
        }
    }
    fatal_startup_error(&format!(
        "Tomari could not open its settings database: {error}"
    ));
}

/// The sidecars SQLite keeps beside a database in WAL mode. A stale `-wal` is
/// *replayed* into whatever database it finds next to it, so leaving one behind
/// would carry the corruption into the replacement.
const DB_SIDECARS: [&str; 2] = ["-wal", "-shm"];

/// The one file operation [`quarantine_database`] needs, behind a trait so each
/// failure ordering can be unit tested without a real corrupt database.
///
/// Renaming is deliberately the only way files are *moved*. Deleting a sidecar
/// would get it out of the way too, but it destroys content — and content SQLite
/// refused to read is exactly what someone may want to recover by hand later. It
/// also cannot be undone, which is what makes the rollback below possible at all.
///
/// `rename` must not replace an existing destination: it fails with
/// `AlreadyExists` instead. A plain `rename(2)` would silently drop whatever was
/// already under a `.broken-`/`.orphaned-` name, and that is the only copy of an
/// earlier reset's evidence.
///
/// `exists` returns a `Result` rather than a bool because `Path::exists` reports
/// a metadata error as "not there". Treating a sidecar we merely *failed to look
/// at* as absent is how one gets left beside a replacement database.
trait FileOps {
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()>;
    fn exists(&self, path: &Path) -> std::io::Result<bool>;
}

struct RealFileOps;

impl FileOps for RealFileOps {
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        rename_no_clobber(from, to)
    }
    fn exists(&self, path: &Path) -> std::io::Result<bool> {
        // `symlink_metadata`, so a dangling symlink counts as present: it still
        // occupies the name a rename would have to go through.
        match std::fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }
}

/// `rename(2)` that refuses to replace an existing destination, atomically —
/// there is no window between an existence check and the move for another writer
/// to slip into.
#[cfg(target_os = "macos")]
fn rename_no_clobber(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_from = CString::new(from.as_os_str().as_bytes())?;
    let c_to = CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both pointers come from `CString`s that outlive the call, and
    // `renamex_np` only reads them.
    let rc = unsafe { libc::renamex_np(c_from.as_ptr(), c_to.as_ptr(), libc::RENAME_EXCL) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Best effort where no exclusive rename exists: check, then move. Tomari does
/// not ship on these platforms; the lock is what rules out the race in practice.
#[cfg(not(target_os = "macos"))]
fn rename_no_clobber(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
    }
    std::fs::rename(from, to)
}

/// Finish a quarantine an earlier launch could not.
///
/// Sidecars beside a database that is *not there* are the signature of a reset
/// interrupted between renames — by a crash, or by a rollback that could not put
/// everything back. SQLite never leaves a WAL without its database, so nothing
/// in normal operation produces that state. It matters because the next
/// `Database::open` would create a fresh database right beside the orphan: SQLite
/// then either replays it into the new file or discards it, and discarding takes
/// with it the only remaining copy of whatever it held.
///
/// Returns whether the path is safe to open — the database is present (so
/// nothing is orphaned), or every orphan was moved aside.
fn sweep_orphan_sidecars(fs: &impl FileOps, db_path: &Path, unix_ms: u128) -> bool {
    let Some(name) = db_path.file_name().and_then(|n| n.to_str()) else {
        tracing::error!(path = %db_path.display(), "the database path has no file name");
        return false;
    };
    match fs.exists(db_path) {
        // The database is where it should be, so any sidecar belongs to it.
        Ok(true) => return true,
        Ok(false) => {}
        Err(e) => {
            // Not knowing is not the same as "no database": creating one now
            // could be creating it beside an orphan.
            tracing::error!(error = %e, path = %db_path.display(), "could not tell whether the database is present");
            return false;
        }
    }
    for suffix in DB_SIDECARS {
        let src = db_path.with_file_name(format!("{name}{suffix}"));
        let result = fs.rename(
            &src,
            &db_path.with_file_name(format!("{name}.orphaned-{unix_ms}{suffix}")),
        );
        if was_absent(&result) {
            continue;
        }
        if let Err(e) = result {
            tracing::error!(
                path = %src.display(),
                error = %e,
                "a sidecar left by an interrupted database reset could not be moved aside"
            );
            return false;
        }
        tracing::warn!(
            path = %src.display(),
            "moved aside a sidecar left behind by an interrupted database reset"
        );
    }
    true
}

/// Whether `result` means "there was no such file", which for a sidecar is the
/// ordinary case rather than a failure.
fn was_absent(result: &std::io::Result<()>) -> bool {
    matches!(result, Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

/// Move the corrupt database and its sidecars aside under a
/// `.broken-<unix-ms>` suffix so a fresh one can take the original path.
/// Returns whether *every* file was moved — only then may the caller reuse that
/// path.
///
/// The database goes first, so the common failure (a directory that cannot be
/// written at all) stops before anything has moved. If a sidecar then cannot be
/// moved, everything already moved is put back: a stale `-wal` beside a
/// brand-new database is replayed into it, so a set that cannot be quarantined
/// whole has to be handed back as it was found — which is also what lets the
/// next launch simply try again.
///
/// Since nothing is ever deleted, the worst case is a rollback that itself
/// fails, or a crash between two renames: the set is then split between its
/// original names and `.broken-<unix-ms>` ones, with every byte still on disk.
/// The next launch recognizes that split — a sidecar with no database — in
/// [`sweep_orphan_sidecars`] and finishes the job before anything is opened.
///
/// Two launches doing this at once is ruled out one level up: `main` takes the
/// [`InstanceLock`] before the database is opened, so only one process can be in
/// here. Without it the second process could move the first's fresh replacement
/// aside and leave one of them writing to a file no longer at the canonical path.
/// The renames themselves never replace an existing file ([`FileOps::rename`]),
/// so a `.broken-` name that is somehow already taken — a stamp collision, or a
/// process the lock does not bind — fails the move rather than destroying what
/// an earlier reset kept.
fn quarantine_database(fs: &impl FileOps, db_path: &Path, unix_ms: u128) -> bool {
    let Some(name) = db_path.file_name().and_then(|n| n.to_str()) else {
        tracing::error!(path = %db_path.display(), "the database path has no file name to move aside");
        return false;
    };
    let aside = |suffix: &str| db_path.with_file_name(format!("{name}.broken-{unix_ms}{suffix}"));

    if let Err(e) = fs.rename(db_path, &aside("")) {
        tracing::error!(error = %e, "could not move the corrupt database aside");
        return false;
    }
    let mut moved = vec![(db_path.to_path_buf(), aside(""))];

    for suffix in DB_SIDECARS {
        let src = db_path.with_file_name(format!("{name}{suffix}"));
        let dst = aside(suffix);
        let result = fs.rename(&src, &dst);
        if was_absent(&result) {
            // No such sidecar: SQLite checkpointed and removed it, which is the
            // usual state after a clean exit.
            continue;
        }
        if let Err(e) = result {
            tracing::error!(
                path = %src.display(),
                error = %e,
                "could not move a database sidecar aside; putting the database back"
            );
            undo_moves(fs, &moved);
            return false;
        }
        moved.push((src, dst));
    }
    true
}

/// Put `moved` back where it came from, newest first — so the sidecars return
/// before the database they belong to.
///
/// Stops at the first failure, and that is the important part. Carrying on would
/// restore the database while leaving a sidecar behind under its `.broken-` name:
/// the next launch would find a database that looks intact, open it without that
/// sidecar, and quietly proceed without whatever it held.
///
/// Stopping gives the invariant [`sweep_orphan_sidecars`] relies on: if a
/// rollback failed at all, the database itself is still aside, so whatever the
/// live sidecars happen to be — none, some, all — the next launch sees no
/// database beside them and finishes the job from there.
fn undo_moves(fs: &impl FileOps, moved: &[(std::path::PathBuf, std::path::PathBuf)]) {
    for (original, aside) in moved.iter().rev() {
        if let Err(e) = fs.rename(aside, original) {
            tracing::error!(
                error = %e,
                path = %aside.display(),
                original = %original.display(),
                "could not put a quarantined database file back; leaving it and everything \
                 under it aside rather than restoring a partial set"
            );
            return;
        }
    }
}

/// Wall-clock milliseconds, used only to name the files set aside.
fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn move_database_aside(paths: &AppPaths) -> bool {
    quarantine_database(&RealFileOps, &paths.db_path, unix_ms())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// The database path the quarantine tests work against, plus its sidecars.
    const DB: &str = "/data/tomari.sqlite";
    const WAL: &str = "/data/tomari.sqlite-wal";
    const SHM: &str = "/data/tomari.sqlite-shm";
    /// A fixed stamp, so the `.broken-` names are predictable.
    const STAMP: u128 = 1_700_000_000_000;

    fn aside(suffix: &str) -> PathBuf {
        PathBuf::from(format!("/data/tomari.sqlite.broken-{STAMP}{suffix}"))
    }

    /// In-memory [`FileOps`] over a set of present paths, with per-path rename
    /// failures so each ordering can be forced. Renaming a path that is not
    /// there reports `NotFound`, exactly as the real filesystem does — which is
    /// what the sidecar handling reads absence off.
    struct FakeFileOps {
        present: RefCell<HashSet<PathBuf>>,
        /// Interior-mutable so a test can lift a failure and carry the same
        /// on-disk state through to the next launch's sweep.
        rename_fails: RefCell<HashSet<PathBuf>>,
        /// Paths whose existence cannot be determined — a metadata error, which
        /// must never read as "absent".
        exists_fails: HashSet<PathBuf>,
    }

    impl FakeFileOps {
        fn with(paths: &[&str]) -> Self {
            Self {
                present: RefCell::new(paths.iter().map(PathBuf::from).collect()),
                rename_fails: RefCell::new(HashSet::new()),
                exists_fails: HashSet::new(),
            }
        }

        /// Make renaming `path` fail — a directory that cannot be written, say.
        fn immovable(self, path: &str) -> Self {
            self.rename_fails.borrow_mut().insert(PathBuf::from(path));
            self
        }

        /// Whatever was blocking renames has cleared.
        fn unblocked(&self) {
            self.rename_fails.borrow_mut().clear();
        }

        /// Make looking `path` up fail, as a metadata error would.
        fn unreadable(mut self, path: &str) -> Self {
            self.exists_fails.insert(PathBuf::from(path));
            self
        }

        fn has(&self, path: &str) -> bool {
            self.present.borrow().contains(&PathBuf::from(path))
        }

        fn kept_aside(&self, suffix: &str) -> bool {
            self.present.borrow().contains(&aside(suffix))
        }

        fn kept_orphaned(&self, suffix: &str) -> bool {
            self.present.borrow().contains(&PathBuf::from(format!(
                "/data/tomari.sqlite.orphaned-{STAMP}{suffix}"
            )))
        }
    }

    impl FileOps for FakeFileOps {
        fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
            if self.rename_fails.borrow().contains(from) {
                return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
            }
            let mut present = self.present.borrow_mut();
            if present.contains(to) {
                return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
            }
            if !present.remove(from) {
                return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
            }
            present.insert(to.to_path_buf());
            Ok(())
        }
        fn exists(&self, path: &Path) -> std::io::Result<bool> {
            if self.exists_fails.contains(path) {
                return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
            }
            Ok(self.present.borrow().contains(path))
        }
    }

    #[test]
    fn sweep_leaves_a_database_and_its_own_sidecars_alone() {
        let fs = FakeFileOps::with(&[DB, WAL, SHM]);
        assert!(sweep_orphan_sidecars(&fs, Path::new(DB), STAMP));
        for path in [DB, WAL, SHM] {
            assert!(fs.has(path), "{path} was touched");
        }
    }

    #[test]
    fn sweep_is_a_noop_before_the_first_launch() {
        let fs = FakeFileOps::with(&[]);
        assert!(sweep_orphan_sidecars(&fs, Path::new(DB), STAMP));
    }

    #[test]
    fn sweep_moves_aside_sidecars_an_interrupted_reset_left_behind() {
        // A sidecar with no database: a reset that stopped between renames, or a
        // rollback that could not finish. Opening a fresh database beside one
        // would have SQLite replay it or delete it, and deleting it takes the
        // only copy left of whatever it held.
        let fs = FakeFileOps::with(&[WAL, SHM]);
        assert!(sweep_orphan_sidecars(&fs, Path::new(DB), STAMP));
        assert!(!fs.has(WAL));
        assert!(!fs.has(SHM));
        assert!(fs.kept_orphaned("-wal"));
        assert!(fs.kept_orphaned("-shm"));
    }

    #[test]
    fn sweep_refuses_to_open_over_an_orphan_it_cannot_move() {
        let fs = FakeFileOps::with(&[WAL]).immovable(WAL);
        assert!(!sweep_orphan_sidecars(&fs, Path::new(DB), STAMP));
        assert!(fs.has(WAL));
    }

    #[test]
    fn sweep_refuses_when_it_cannot_tell_whether_the_database_is_there() {
        // `Path::exists` would answer "absent" here; that is how an orphan ends
        // up beside a brand-new database.
        let fs = FakeFileOps::with(&[DB, WAL]).unreadable(DB);
        assert!(!sweep_orphan_sidecars(&fs, Path::new(DB), STAMP));
        assert!(fs.has(WAL), "nothing may move on an unreadable lookup");
    }

    #[test]
    fn quarantine_moves_the_database_and_its_sidecars() {
        let fs = FakeFileOps::with(&[DB, WAL, SHM]);
        assert!(quarantine_database(&fs, Path::new(DB), STAMP));
        for original in [DB, WAL, SHM] {
            assert!(!fs.has(original), "{original} was left behind");
        }
        for suffix in ["", "-wal", "-shm"] {
            assert!(fs.kept_aside(suffix), "{suffix:?} was not kept aside");
        }
    }

    #[test]
    fn quarantine_never_renames_over_an_existing_quarantine() {
        // The `-wal` destination is already taken, so the sidecar move fails and
        // the database that was already moved comes back — nothing replaced.
        let taken = aside("-wal");
        let fs = FakeFileOps::with(&[DB, WAL, SHM, taken.to_str().unwrap()]);
        assert!(!quarantine_database(&fs, Path::new(DB), STAMP));
        for path in [DB, WAL, SHM] {
            assert!(fs.has(path), "{path} must stay in place");
        }
        assert!(fs.has(taken.to_str().unwrap()));
    }

    #[test]
    fn real_rename_refuses_to_replace_an_existing_file() {
        let dir = std::env::temp_dir().join(format!("tomari-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let from = dir.join("from");
        let to = dir.join("to");
        std::fs::write(&from, b"new").unwrap();
        std::fs::write(&to, b"kept").unwrap();
        let err = RealFileOps.rename(&from, &to).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&to).unwrap(), b"kept");
        std::fs::remove_file(&to).unwrap();
        RealFileOps.rename(&from, &to).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quarantine_needs_no_sidecars_to_be_present() {
        // The usual state after a clean exit: SQLite checkpointed and removed
        // them, so renaming reports `NotFound` and that is not a failure.
        let fs = FakeFileOps::with(&[DB]);
        assert!(quarantine_database(&fs, Path::new(DB), STAMP));
        assert!(!fs.has(DB));
        assert!(fs.kept_aside(""));
    }

    #[test]
    fn quarantine_touches_nothing_when_the_database_cannot_be_moved() {
        // The common failure — a directory that cannot be written — is hit on
        // the very first rename, before any sidecar has moved.
        let fs = FakeFileOps::with(&[DB, WAL, SHM]).immovable(DB);
        assert!(!quarantine_database(&fs, Path::new(DB), STAMP));
        for original in [DB, WAL, SHM] {
            assert!(fs.has(original), "{original} should not have been touched");
        }
        for suffix in ["", "-wal", "-shm"] {
            assert!(!fs.kept_aside(suffix), "{suffix:?} should not exist");
        }
    }

    #[test]
    fn quarantine_puts_the_database_back_when_a_sidecar_cannot_be_moved() {
        // A stale `-wal` beside a brand-new database is replayed into it, so a
        // set that cannot be quarantined whole is handed back as it was found.
        let fs = FakeFileOps::with(&[DB, WAL, SHM]).immovable(WAL);
        assert!(!quarantine_database(&fs, Path::new(DB), STAMP));
        for original in [DB, WAL, SHM] {
            assert!(fs.has(original), "{original} was not put back");
        }
        for suffix in ["", "-wal", "-shm"] {
            assert!(
                !fs.kept_aside(suffix),
                "{suffix:?} should not be left aside"
            );
        }
    }

    #[test]
    fn quarantine_puts_an_already_moved_sidecar_back_too() {
        // The rollback has to undo the whole run, not just the database: here
        // `-wal` moved before `-shm` failed.
        let fs = FakeFileOps::with(&[DB, WAL, SHM]).immovable(SHM);
        assert!(!quarantine_database(&fs, Path::new(DB), STAMP));
        assert!(
            fs.has(WAL),
            "the sidecar that had already moved was not put back"
        );
        assert!(fs.has(DB));
        assert!(fs.has(SHM));
        for suffix in ["", "-wal", "-shm"] {
            assert!(
                !fs.kept_aside(suffix),
                "{suffix:?} should not be left aside"
            );
        }
    }

    #[test]
    fn quarantine_reports_a_rollback_it_could_not_finish() {
        // Nothing is deleted, so the worst case is a split set: the database is
        // still aside under its `.broken-` name, and the run still reports
        // failure so no fresh database is created on top of it.
        let fs = FakeFileOps::with(&[DB, WAL])
            .immovable(WAL)
            .immovable(&aside("").to_string_lossy());
        assert!(!quarantine_database(&fs, Path::new(DB), STAMP));
        assert!(!fs.has(DB));
        assert!(fs.kept_aside(""));
        assert!(fs.has(WAL));
    }

    #[test]
    fn quarantine_leaves_the_database_aside_when_a_sidecar_cannot_come_back() {
        // `-shm` cannot be quarantined, and putting `-wal` back then fails too.
        // Restoring the database anyway would leave one that *looks* intact next
        // launch, to be opened without the `-wal` still sitting aside. Stopping
        // keeps the live names consistent — nothing but the untouched `-shm` —
        // which the orphan sweep finishes from.
        let fs = FakeFileOps::with(&[DB, WAL, SHM])
            .immovable(SHM)
            .immovable(&aside("-wal").to_string_lossy());
        assert!(!quarantine_database(&fs, Path::new(DB), STAMP));
        assert!(
            !fs.has(DB),
            "the database must not come back without its -wal"
        );
        assert!(fs.kept_aside(""));
        assert!(fs.kept_aside("-wal"));
        assert!(fs.has(SHM));

        // While whatever blocked `-shm` persists, the next launch refuses rather
        // than opening a fresh database beside it.
        assert!(!sweep_orphan_sidecars(&fs, Path::new(DB), STAMP));

        // Once it clears, the very same on-disk state is the signature the sweep
        // acts on: no database beside the sidecar that was left.
        fs.unblocked();
        assert!(sweep_orphan_sidecars(&fs, Path::new(DB), STAMP));
        assert!(!fs.has(SHM));
        assert!(fs.kept_orphaned("-shm"));
    }

    #[test]
    fn quarantine_leaves_the_database_aside_when_only_it_cannot_come_back() {
        // The other rollback ordering: `-wal` goes back fine, the database does
        // not. It is still aside, which is what the sweep needs to see — the
        // live sidecars being a complete set makes no difference, because it is
        // the missing database that the signature is read off.
        let fs = FakeFileOps::with(&[DB, WAL, SHM])
            .immovable(SHM)
            .immovable(&aside("").to_string_lossy());
        assert!(!quarantine_database(&fs, Path::new(DB), STAMP));
        assert!(fs.has(WAL), "the sidecar came back");
        assert!(
            fs.has(SHM),
            "the sidecar that blocked the reset never moved"
        );
        assert!(!fs.has(DB));
        assert!(fs.kept_aside(""));

        // No database beside those sidecars, so the next launch clears them
        // rather than opening a fresh database into them.
        fs.unblocked();
        assert!(sweep_orphan_sidecars(&fs, Path::new(DB), STAMP));
        assert!(!fs.has(WAL));
        assert!(!fs.has(SHM));
        assert!(fs.kept_orphaned("-wal"));
        assert!(fs.kept_orphaned("-shm"));
    }

    #[test]
    fn quarantine_fails_on_a_path_with_no_file_name() {
        let fs = FakeFileOps::with(&[]);
        assert!(!quarantine_database(&fs, Path::new("/"), STAMP));
    }

    #[test]
    fn a_pristine_database_seeds_and_counts_as_a_first_run() {
        let db = Database::open_in_memory().unwrap();

        assert!(seed_first_run_defaults(&db));
        assert!(db.settings_exist().unwrap());
        assert!(db.count_hotkeys().unwrap() > 0);
        assert!(db.count_modifier_rules().unwrap() > 0);
    }

    #[test]
    fn an_initialized_database_is_not_a_first_run() {
        let db = Database::open_in_memory().unwrap();
        seed_first_run_defaults(&db);

        // The same database on its next launch: settings row present.
        assert!(!seed_first_run_defaults(&db));
    }

    #[test]
    fn stray_hotkeys_without_settings_skip_the_seed_and_the_first_run() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_hotkey(&defaults::default_hotkeys()[0]).unwrap();

        assert!(!seed_first_run_defaults(&db));
        assert!(!db.settings_exist().unwrap(), "nothing was seeded");
        assert_eq!(db.count_hotkeys().unwrap(), 1, "the stray row was kept");
    }

    #[test]
    fn stray_modifier_rules_without_settings_skip_the_seed_and_the_first_run() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_modifier_rule(&defaults::default_modifier_rules()[0])
            .unwrap();

        assert!(!seed_first_run_defaults(&db));
        assert!(!db.settings_exist().unwrap(), "nothing was seeded");
    }
}
