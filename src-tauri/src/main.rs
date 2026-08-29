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
mod lifecycle;
mod locks;
mod logcap;
mod mailbox;
mod menubar;
#[cfg(target_os = "macos")]
mod overlay;
mod ratelimit;
mod recovery_markers;
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
use tomari_core::{AppPaths, AppSettings, Database, PersistedSettings, defaults};
use tomari_keyboard::ModifierEngine;
use tomari_window::WindowManager;

use crate::instance_lock::{AcquireError, InstanceLock, Outcome};
use crate::locks::MutexExt;
use crate::state::AppState;

/// Apply a permission transition only while its delayed main-thread callback
/// still belongs to the running app. The poller samples TCC off-main, so quit
/// can become terminal before this callback reaches AppKit's queue.
#[cfg(any(target_os = "macos", test))]
fn apply_permission_transition_if_running(
    lifecycle: &lifecycle::AppLifecycle,
    apply: impl FnOnce(),
) -> bool {
    if !lifecycle.is_running() {
        return false;
    }
    apply();
    true
}

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
        .manage(paths)
        .manage(app_state)
        .manage(commands::PendingUpdate::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::retry_settings_recovery,
            commands::reset_settings_recovery,
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
            commands::check_for_update,
            commands::install_update,
            commands::get_keep_awake,
            commands::set_keep_awake,
            commands::configure_keep_awake,
            commands::cancel_keep_awake_transition,
            commands::retry_keep_awake_transition,
            commands::dismiss_keep_awake_recovery,
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
            let configuration_recovery = state.configuration_recovery_required();
            let startup_automation = startup_automation_plan(state.inner());

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

            // Apply trusted menu-bar and login-item preferences on launch so
            // the actual system state matches what the settings show. Recovery
            // keeps the tray visible but must not rewrite the user's login item
            // from a synthetic fail-closed profile.
            let (show_tray, launch_at_login) = {
                let s = state.settings.lock_safe();
                (s.show_in_menu_bar, s.launch_at_login)
            };
            if !show_tray {
                tray::set_visible(&handle, false);
            }
            if !configuration_recovery {
                commands::apply_launch_at_login(&handle, launch_at_login);
            }

            // Individual hotkeys that fail to register are logged (and
            // tolerated) inside `register_all`; only failing to read the
            // hotkey list at all lands here.
            if startup_automation.keyboard
                && let Err(e) = shortcuts::register_all(&handle, state.inner())
            {
                tracing::error!(error = %e, "failed to register global shortcuts");
            }

            // Start the keyboard event tap (Input Monitoring) only for a
            // trusted, enabled configuration. On an ordinary launch this is
            // attempted even before permission is granted so Tomari appears in
            // the Input Monitoring list; recovery never attempts the tap.
            #[cfg(target_os = "macos")]
            if startup_automation.keyboard {
                eventtap::restart(&handle);
            } else {
                // A prior crash may have left Caps Lock mapped outside this
                // process. Restore it without ever attempting to create a tap.
                let _ = capsmap::reconcile(false);
            }

            // Prime the drag-to-snap display-geometry cache and keep it current
            // on display changes — before the drag-to-snap tap starts, so the
            // first drag always has geometry to snap against.
            #[cfg(target_os = "macos")]
            displays::install(&handle);

            // Start the drag-to-snap and drag-to-move taps when enabled.
            #[cfg(target_os = "macos")]
            if startup_automation.drag_to_snap {
                drag_to_snap::restart(&handle);
            }
            #[cfg(target_os = "macos")]
            if startup_automation.drag_to_move {
                drag_to_move::restart(&handle);
            }

            // A sleep or session switch can swallow key releases; reset the
            // key-tracking state whenever the system comes back.
            #[cfg(target_os = "macos")]
            wake::install(&handle);

            // Keep-awake never persists as "on". A lid-close sleep override a
            // previous run left behind after an unclean exit is not cleared
            // here on the marker's evidence alone — it is surfaced for the
            // user to decide (see `keepawake::reconcile_on_launch`).
            keepawake::reconcile_on_launch(&handle);
            // The tray was built before the reconcile ran; rebuild it so a
            // leftover override's undecided state disables the item at once.
            tray::refresh(&handle);
            keepawake::start_monitor(&handle);

            // Put the menu bar divider back if tidying is switched on. Always
            // collapsed to start with, so a launch looks the same every time.
            if startup_automation.menu_bar_tidy {
                menubar::init(&handle);
            }

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
            if !configuration_recovery {
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
                let poll_lifecycle = std::sync::Arc::clone(&state.lifecycle);
                let spawned =
                    poll_lifecycle.spawn_tracked("tomari-permission-poller", move |lifecycle| {
                        // Poll responsively while a permission is still missing, then
                        // ease off to a slow heartbeat once both are granted and
                        // stable — there is nothing left to react to but the rare
                        // revocation, so a 2 s spin would be pure waste.
                        const FAST: std::time::Duration = std::time::Duration::from_secs(2);
                        const SLOW: std::time::Duration = std::time::Duration::from_secs(30);
                        let mut last = Some(initial);
                        let mut interval = if initial == (true, true) { SLOW } else { FAST };
                        loop {
                            if lifecycle.wait_for_shutdown(interval) {
                                return;
                            }
                            let current = tray::permission_state(&poll_handle);
                            if !lifecycle.is_running() {
                                return;
                            }
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
                            // A revoke rebuilds them too: the start then fails
                            // and each tap records `PermissionDenied` — its true
                            // state — instead of a handle to a tap the system
                            // will no longer feed, which the settings check would
                            // otherwise keep reporting as running.
                            let input_monitoring_changed =
                                matches!(last, Some((_, was_im)) if was_im != current.1);
                            last = Some(current);
                            let refresh_handle = poll_handle.clone();
                            let refresh_version = poll_version.clone();
                            let _ = poll_handle.run_on_main_thread(move || {
                                let Some(state) = refresh_handle.try_state::<AppState>() else {
                                    return;
                                };
                                let _ = apply_permission_transition_if_running(
                                    &state.lifecycle,
                                    || {
                                        if input_monitoring_changed
                                            && !state.configuration_recovery_required()
                                        {
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
                                                revision: commands::next_permission_revision(),
                                            },
                                        );
                                        // Keep the stored snapshot tracking every observed
                                        // transition, so the next launch compares against
                                        // the state this run actually ended with.
                                        if !state.configuration_recovery_required() {
                                            regrant::store_snapshot(
                                                &state.db,
                                                current,
                                                &refresh_version,
                                            );
                                        }
                                    },
                                );
                            });
                        }
                    });
                if let Err(error) = spawned {
                    tracing::warn!(%error, "could not start the permission poller");
                }
            }

            // A true first run and every recovery session open the settings
            // window. A successful repair carries a durable one-shot intent
            // through the process relaunch too; consume it only after show
            // succeeds, so an Accessory app with no Dock icon never strands
            // the user behind a hidden panel. This is safe before the WebView
            // finishes loading because it only shows the existing window.
            let paths = app.state::<AppPaths>();
            if let Err(error) = show_startup_panel_if_requested(
                &paths,
                state.first_run,
                configuration_recovery,
                || actions::show_panel(&handle).map_err(|error| error.to_string()),
            ) {
                tracing::error!(%error, "could not complete the startup panel request");
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
                // Only a hide that took effect idles the monitor; a panel still
                // on screen must keep its status fresh.
                if window.hide().is_ok() {
                    keepawake::set_panel_visible(false);
                }
            }
        })
        .build(context)
        // Startup must not panic (see `build_state`'s doc comment): `.expect`
        // here would be exactly that, an invisible crash loop for a login-item
        // Accessory with no Dock icon or terminal. Route a build failure
        // through the same native-alert-and-exit path as every other
        // unrecoverable startup error instead.
        .unwrap_or_else(|e| fatal_startup_error(&format!("Tomari could not start: {e}")))
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
                lifecycle::handle_exit_requested(app, code, &api);
            }
        });
}

/// Resolve a `tomari://` URL to an action and run it. Fire-and-forget: the
/// launcher has already moved on, so there is no caller to return a result to —
/// a malformed URL, a disabled master switch, or a failed action is logged and
/// dropped rather than surfaced.
///
/// The URL itself is never logged: any local process can send one, and its
/// query, userinfo or path may carry tokens or personal data that would then
/// sit in the seven-day log. Only the action kind and a redacted reason are
/// recorded, and at most one line per [`DEEP_LINK_LOG_INTERVAL`], so a sender
/// spraying URLs cannot fill the log either.
fn dispatch_deep_link(app: &tauri::AppHandle, raw: &str) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let external = match tomari_core::parse_deep_link(raw) {
        Ok(action) => action,
        Err(e) => {
            if DEEP_LINK_LOG.allow(std::time::Instant::now()) {
                tracing::warn!(reason = e.kind(), "ignoring malformed tomari:// URL");
            }
            return;
        }
    };
    // Window placement is gated behind the master switch, so an external
    // process cannot move the user's windows when they have opted out.
    // `toggle-panel` is exempt: it only shows/hides Tomari's own panel and is
    // the recovery route for a hidden menu bar, so it must keep working.
    if external.is_window_placement() && !state.settings.lock_safe().external_window_actions_enabled
    {
        if DEEP_LINK_LOG.allow(std::time::Instant::now()) {
            tracing::warn!(
                action = ?external,
                "external window actions disabled; ignoring tomari:// URL"
            );
        }
        return;
    }
    // dispatch does exactly what the action says — a snap never summons the
    // panel — so Tomari does not steal frontmost from the window being placed.
    let action: tomari_core::AppAction = external.into();
    // The URL grammar only yields closed-enum actions today; validating here
    // anyway keeps every path an action can arrive by behind one validator.
    let action = match validate::sanitize_app_action(action) {
        Ok(action) => action,
        Err(e) => {
            // The action is a closed enum; the error's text is not logged —
            // nothing free-form from a URL-derived path reaches the file.
            let _ = e;
            if DEEP_LINK_LOG.allow(std::time::Instant::now()) {
                tracing::warn!(action = ?external, "tomari:// action rejected");
            }
            return;
        }
    };
    if let Err(e) = actions::dispatch(&action, app, state.inner())
        && DEEP_LINK_LOG.allow(std::time::Instant::now())
    {
        // Same rule: the failure's kind (its error code) is logged, never its
        // free-form message.
        tracing::warn!(action = ?external, code = ?e.code, "tomari:// action failed");
    }
}

/// How often `tomari://` handling may log at most.
const DEEP_LINK_LOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
static DEEP_LINK_LOG: ratelimit::RateLimit = ratelimit::RateLimit::new(DEEP_LINK_LOG_INTERVAL);

/// How many daily log files to keep before the oldest is pruned.
const LOG_KEEP_FILES: usize = 7;
/// The per-day byte budget of the log file.
static LOG_BUDGET: logcap::DailyBudget = logcap::DailyBudget::new(logcap::DAILY_LOG_BYTES);

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
        let logs_dir = p.data_dir.join("logs");
        // What an earlier run wrote today counts against today's budget too;
        // otherwise every restart would hand the day a fresh cap.
        LOG_BUDGET.seed(
            logcap::today(),
            logcap::existing_bytes_today(&logs_dir, "tomari", "log"),
        );
        match tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("tomari")
            .filename_suffix("log")
            .max_log_files(LOG_KEEP_FILES)
            .build(logs_dir)
        {
            // Capped per day (see `logcap`): rotation bounds how many days are
            // kept, the budget bounds how much one day can hold.
            Ok(appender) => Some(
                tracing_subscriber::fmt::layer()
                    .with_writer(logcap::Capped::new(appender, &LOG_BUDGET))
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

struct OpenedDatabase {
    db: Database,
    recovered_from_corruption: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Initialization {
    Existing,
    FirstRun,
    MissingSettingsWithData,
}

struct ReadyConfiguration {
    settings: AppSettings,
    rules: Vec<tomari_core::ModifierRule>,
    first_run: bool,
    dropped_rules: usize,
    dropped_hotkeys: usize,
}

enum StartupConfiguration {
    Ready(ReadyConfiguration),
    RecoveryRequired(crate::state::ConfigurationRecovery),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StartupAutomationPlan {
    keyboard: bool,
    drag_to_snap: bool,
    drag_to_move: bool,
    menu_bar_tidy: bool,
}

fn startup_automation_plan(state: &AppState) -> StartupAutomationPlan {
    let settings = state.settings.lock_safe();
    StartupAutomationPlan {
        keyboard: settings.keyboard_enabled,
        drag_to_snap: settings.window_management_enabled && settings.drag_to_snap_enabled,
        drag_to_move: settings.window_management_enabled && settings.drag_to_move_enabled,
        menu_bar_tidy: settings.menu_bar_tidy_enabled,
    }
}

fn should_show_startup_panel(
    first_run: bool,
    configuration_recovery: bool,
    show_after_recovery: bool,
) -> bool {
    first_run || configuration_recovery || show_after_recovery
}

fn show_startup_panel_if_requested(
    paths: &AppPaths,
    first_run: bool,
    configuration_recovery: bool,
    show: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let (show_after_recovery, marker_error) =
        match recovery_markers::show_panel_after_recovery(paths) {
            Ok(requested) => (requested, None),
            Err(error) => {
                // Losing a one-shot marker read must not hide the only route
                // back to the settings that recovery just disabled.
                (true, Some(error.to_string()))
            }
        };
    if !should_show_startup_panel(first_run, configuration_recovery, show_after_recovery) {
        return Ok(());
    }

    show()?;
    if let Some(error) = marker_error {
        return Err(format!(
            "the panel was shown, but its recovery marker could not be inspected: {error}"
        ));
    }
    if show_after_recovery {
        recovery_markers::clear_show_panel_after_recovery(paths).map_err(|error| {
            format!("the panel was shown, but its recovery marker remains: {error}")
        })?;
    }
    Ok(())
}

/// Open the database, establish one complete persisted snapshot, and assemble
/// shared state. Any uncertainty is an interlock, never an excuse to activate
/// defaults: the degraded state keeps every input/window effect off, exposes
/// only explicit retry/reset UI, and rejects ordinary configuration writes.
///
/// Startup must not panic: as an Accessory (no Dock icon, no window) launched
/// at login, a panic is a silent crash loop with no feedback at all. Anything
/// unrecoverable shows a native alert once and exits instead.
fn build_state(paths: &AppPaths) -> AppState {
    let database_reset_marker = match recovery_markers::database_reset_required(paths) {
        Ok(required) => required,
        Err(error) => fatal_startup_error(&format!(
            "Tomari could not inspect its database recovery marker: {error}"
        )),
    };
    let OpenedDatabase {
        mut db,
        mut recovered_from_corruption,
    } = open_database(paths);
    recovered_from_corruption |= database_reset_marker;

    loop {
        if recovered_from_corruption {
            return recovery_state(db, crate::state::ConfigurationRecovery::DatabaseReset);
        }

        match load_startup_configuration(&db) {
            Ok(StartupConfiguration::Ready(ready)) => {
                warn_on_undecodable_rows(ready.dropped_rules, ready.dropped_hotkeys);
                return AppState::new(
                    db,
                    ModifierEngine::new(ready.rules),
                    make_window_manager(),
                    ready.settings,
                    ready.first_run,
                );
            }
            Ok(StartupConfiguration::RecoveryRequired(recovery)) => {
                return recovery_state(db, recovery);
            }
            Err(error) if error.is_database_corruption() => {
                // Close the live WAL connection before moving its database and
                // sidecars. Letting it drop after the rename could checkpoint
                // into a path that now belongs to the replacement.
                drop(db);
                db = recover_corrupt_database(paths, &error);
                recovered_from_corruption = true;
            }
            Err(error) => {
                let recovery = if matches!(error, tomari_core::Error::Serde(_)) {
                    crate::state::ConfigurationRecovery::SettingsUnreadable
                } else {
                    crate::state::ConfigurationRecovery::DatabaseReadFailed
                };
                tracing::error!(%error, "persisted configuration is unreadable; automation is paused");
                return recovery_state(db, recovery);
            }
        }
    }
}

fn recovery_state(db: Database, recovery: crate::state::ConfigurationRecovery) -> AppState {
    AppState::new_with_configuration_recovery(
        db,
        ModifierEngine::new(Vec::new()),
        make_window_manager(),
        AppSettings::fail_closed(),
        false,
        Some(recovery),
    )
}

fn load_startup_configuration(db: &Database) -> Result<StartupConfiguration, tomari_core::Error> {
    let initialization = seed_first_run_defaults(db)?;
    if initialization == Initialization::MissingSettingsWithData {
        tracing::error!(
            "settings row is missing while other configuration remains; automation is paused"
        );
        return Ok(StartupConfiguration::RecoveryRequired(
            crate::state::ConfigurationRecovery::SettingsUnreadable,
        ));
    }

    // Read every persisted source under one SQLite snapshot before any input
    // or window effect is activated. Meta and placement values participate in
    // the hard-read boundary even though only keyboard rows are loaded into an
    // engine here; missing schema or scalar damage anywhere must fail closed.
    let report = db.preflight_persisted_state()?;
    let settings = match report.settings {
        PersistedSettings::Ready(settings)
            if report.settings_rows.stored == 1 && report.settings_rows.skipped == 0 =>
        {
            validate::repair_settings(settings)
        }
        PersistedSettings::Missing | PersistedSettings::UnreadableJson { .. } => {
            tracing::error!(
                "the canonical settings row is missing or unreadable; automation is paused"
            );
            return Ok(StartupConfiguration::RecoveryRequired(
                crate::state::ConfigurationRecovery::SettingsUnreadable,
            ));
        }
        PersistedSettings::Ready(_) => {
            tracing::error!(
                "the settings table is not a complete canonical snapshot; automation is paused"
            );
            return Ok(StartupConfiguration::RecoveryRequired(
                crate::state::ConfigurationRecovery::SettingsUnreadable,
            ));
        }
    };
    let mut rules = report.modifier_rules;
    let dropped_rules = report.modifier_rule_rows.skipped;
    let dropped_hotkeys = report.hotkey_rows.skipped;

    if settings.command_ime_switch_enabled {
        rules.extend(defaults::command_ime_rules());
    }
    Ok(StartupConfiguration::Ready(ReadyConfiguration {
        settings,
        rules,
        first_run: initialization == Initialization::FirstRun,
        dropped_rules,
        dropped_hotkeys,
    }))
}

/// Seed defaults only on the very first run, detected by the absence of the
/// settings row (plus an otherwise-empty database, checked below). The
/// settings row — not empty tables — is the primary marker so that a user who
/// deliberately clears all of their hotkeys or rules does not get them back.
///
/// Returns whether this is an existing store, a seed that just completed, or
/// an inconsistent store with user data but no settings row. Read and write
/// errors propagate so corruption can re-enter the central quarantine path and
/// every other failure can activate the recovery interlock without touching
/// the user's data.
fn seed_first_run_defaults(db: &Database) -> Result<Initialization, tomari_core::Error> {
    let report = db.preflight_persisted_state()?;
    match report.settings {
        // Any canonical settings row, even one that this build cannot decode,
        // proves the store is not a first run. The complete startup load below
        // decides whether it is safe to activate.
        PersistedSettings::Ready(_) | PersistedSettings::UnreadableJson { .. } => {
            Ok(Initialization::Existing)
        }
        // No settings row — a first run *if* the database is otherwise empty.
        // Guard against seeding over any inconsistent database that already has
        // persisted state. Besides shortcuts and rules, internal metadata and
        // remembered window placements can predate a failed settings write.
        // Only seed a truly pristine database; a probe failure propagates into
        // the recovery interlock, never risking a clobber.
        PersistedSettings::Missing => {
            if !report.is_pristine() {
                tracing::warn!(
                    "settings row missing but other persisted data exists; skipping first-run seed to avoid overwriting existing data"
                );
                return Ok(Initialization::MissingSettingsWithData);
            }
            db.seed_defaults(
                &defaults::default_hotkeys(),
                &defaults::default_modifier_rules(),
                &AppSettings::default(),
            )?;
            Ok(Initialization::FirstRun)
        }
    }
}

/// Alert (once) when the database holds hotkey or rule rows that no longer
/// decode — which the list queries skip silently — so a vanished shortcut or
/// rule is visible rather than a mystery. The counts belong to the same full
/// startup read as the decoded rows; a hard count failure activates recovery
/// before this notification path is reached.
fn warn_on_undecodable_rows(rules_dropped: usize, hotkeys_dropped: usize) {
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
fn open_database(paths: &AppPaths) -> OpenedDatabase {
    if !sweep_orphan_sidecars(&RealFileOps, &paths.db_path, unix_ms()) {
        fatal_startup_error(&format!(
            "Tomari found -wal or -shm files left beside {} by an interrupted settings \
             reset, and could not move them aside. Move them somewhere else, then open \
             Tomari again.",
            paths.db_path.display()
        ));
    }
    let error = match Database::open(&paths.db_path) {
        Ok(db) => {
            return OpenedDatabase {
                db,
                recovered_from_corruption: false,
            };
        }
        Err(e) => e,
    };
    if error.is_database_corruption() {
        return OpenedDatabase {
            db: recover_corrupt_database(paths, &error),
            recovered_from_corruption: true,
        };
    }
    fatal_startup_error(&format!(
        "Tomari could not open its settings database: {error}"
    ));
}

fn recover_corrupt_database(paths: &AppPaths, error: &tomari_core::Error) -> Database {
    recover_corrupt_database_with(paths, error, |message| alert(message, false))
}

fn recover_corrupt_database_with(
    paths: &AppPaths,
    error: &tomari_core::Error,
    notify: impl FnOnce(&str),
) -> Database {
    tracing::error!(%error, "database is corrupt — moving it aside and pausing automation");
    let moved = match quarantine_after_recovery_marker(
        paths,
        recovery_markers::arm_database_reset_required,
        || move_database_aside(paths),
    ) {
        Ok(moved) => moved,
        Err(marker_error) => fatal_startup_error(&format!(
            "Tomari's settings database is damaged, but the recovery marker could not be written: \
             {marker_error}. The database was left in place."
        )),
    };
    if !moved {
        // The corrupt set is still in place — either the database, or a
        // sidecar SQLite would replay into whatever replaced it. A "fresh"
        // database beside those would not be fresh, so name the files to deal
        // with rather than start on top of them.
        fatal_startup_error(&format!(
            "Tomari's settings database is damaged, and could not be moved aside \
             automatically. Move {} — along with any -wal or -shm file beside it \
             — somewhere else, then open Tomari again.",
            paths.db_path.display()
        ));
    }
    match Database::open(&paths.db_path) {
        Ok(db) => {
            notify(
                "Tomari found a damaged settings database and paused all keyboard and \
                 window automation. The unreadable file was kept next to it with a \
                 .broken suffix. Open Tomari to review the safe reset.",
            );
            db
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

fn quarantine_after_recovery_marker(
    paths: &AppPaths,
    arm_marker: impl FnOnce(&AppPaths) -> std::io::Result<()>,
    quarantine: impl FnOnce() -> bool,
) -> std::io::Result<bool> {
    arm_marker(paths)?;
    Ok(quarantine())
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
    use std::sync::{Arc, mpsc};

    /// The database path the quarantine tests work against, plus its sidecars.
    const DB: &str = "/data/tomari.sqlite";
    const WAL: &str = "/data/tomari.sqlite-wal";
    const SHM: &str = "/data/tomari.sqlite-shm";
    /// A fixed stamp, so the `.broken-` names are predictable.
    const STAMP: u128 = 1_700_000_000_000;

    fn aside(suffix: &str) -> PathBuf {
        PathBuf::from(format!("/data/tomari.sqlite.broken-{STAMP}{suffix}"))
    }

    #[test]
    fn queued_permission_transition_is_dropped_after_terminal_quit() {
        let lifecycle = Arc::new(lifecycle::AppLifecycle::default());
        let (queued_tx, queued_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (applied_tx, applied_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let callback_lifecycle = Arc::clone(&lifecycle);
        let callback = std::thread::spawn(move || {
            queued_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let applied = apply_permission_transition_if_running(&callback_lifecycle, || {
                applied_tx.send(()).unwrap();
            });
            result_tx.send(applied).unwrap();
        });

        queued_rx.recv().unwrap();
        lifecycle.stop_for_test();
        release_tx.send(()).unwrap();

        callback.join().unwrap();
        assert!(!result_rx.recv().unwrap());
        assert!(applied_rx.try_recv().is_err());
    }

    #[test]
    fn startup_panel_policy_includes_the_one_shot_recovery_intent() {
        assert!(!should_show_startup_panel(false, false, false));
        assert!(should_show_startup_panel(true, false, false));
        assert!(should_show_startup_panel(false, true, false));
        assert!(should_show_startup_panel(false, false, true));
    }

    #[test]
    fn successful_recovery_panel_show_consumes_its_one_shot_intent() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(directory.path());
        recovery_markers::arm_show_panel_after_recovery(&paths).unwrap();
        let shows = std::cell::Cell::new(0);

        show_startup_panel_if_requested(&paths, false, false, || {
            shows.set(shows.get() + 1);
            Ok(())
        })
        .unwrap();
        show_startup_panel_if_requested(&paths, false, false, || {
            shows.set(shows.get() + 1);
            Ok(())
        })
        .unwrap();

        assert_eq!(shows.get(), 1);
        assert!(!recovery_markers::show_panel_after_recovery(&paths).unwrap());
    }

    #[test]
    fn failed_recovery_panel_show_keeps_its_intent() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(directory.path());
        recovery_markers::arm_show_panel_after_recovery(&paths).unwrap();

        assert!(
            show_startup_panel_if_requested(&paths, false, false, || {
                Err("window unavailable".into())
            })
            .is_err()
        );
        assert!(recovery_markers::show_panel_after_recovery(&paths).unwrap());
    }

    #[test]
    fn quarantine_never_starts_before_the_recovery_marker_is_armed() {
        let paths = AppPaths::with_root("/data");
        let quarantine_called = std::cell::Cell::new(false);
        let result = quarantine_after_recovery_marker(
            &paths,
            |_| Err(std::io::Error::other("marker write failed")),
            || {
                quarantine_called.set(true);
                true
            },
        );

        assert!(result.is_err());
        assert!(!quarantine_called.get());
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

        assert!(
            !db.has_persisted_data().unwrap(),
            "migrated schema and user_version are not persisted app data"
        );
        assert_eq!(
            seed_first_run_defaults(&db).unwrap(),
            Initialization::FirstRun
        );
        assert!(db.has_persisted_data().unwrap());
        assert!(db.settings_exist().unwrap());
        assert!(db.count_hotkeys().unwrap() > 0);
        assert!(db.count_modifier_rules().unwrap() > 0);
    }

    #[test]
    fn an_initialized_database_is_not_a_first_run() {
        let db = Database::open_in_memory().unwrap();
        seed_first_run_defaults(&db).unwrap();

        // The same database on its next launch: settings row present.
        assert_eq!(
            seed_first_run_defaults(&db).unwrap(),
            Initialization::Existing
        );
    }

    #[test]
    fn startup_preflight_requires_metadata_and_placement_tables() {
        for table in ["meta", "window_placements"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("settings.sqlite");
            {
                let db = Database::open(&path).unwrap();
                seed_first_run_defaults(&db).unwrap();
            }
            {
                let conn = rusqlite::Connection::open(&path).unwrap();
                conn.execute(&format!("DROP TABLE {table}"), []).unwrap();
            }
            let db = Database::open(&path).unwrap();

            let error = match load_startup_configuration(&db) {
                Err(error) => error,
                Ok(_) => panic!("{table}: incomplete schema must not become ready"),
            };
            assert!(matches!(error, tomari_core::Error::Database(_)));

            let state = recovery_state(db, crate::state::ConfigurationRecovery::DatabaseReadFailed);
            assert_eq!(
                startup_automation_plan(&state),
                StartupAutomationPlan {
                    keyboard: false,
                    drag_to_snap: false,
                    drag_to_move: false,
                    menu_bar_tidy: false,
                },
                "{table}: structural recovery must remain fail-closed"
            );
        }
    }

    #[test]
    fn stray_hotkeys_without_settings_skip_the_seed_and_the_first_run() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_hotkey(&defaults::default_hotkeys()[0]).unwrap();

        assert_eq!(
            seed_first_run_defaults(&db).unwrap(),
            Initialization::MissingSettingsWithData
        );
        assert!(!db.settings_exist().unwrap(), "nothing was seeded");
        assert_eq!(db.count_hotkeys().unwrap(), 1, "the stray row was kept");
    }

    #[test]
    fn stray_modifier_rules_without_settings_skip_the_seed_and_the_first_run() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_modifier_rule(&defaults::default_modifier_rules()[0])
            .unwrap();

        assert_eq!(
            seed_first_run_defaults(&db).unwrap(),
            Initialization::MissingSettingsWithData
        );
        assert!(!db.settings_exist().unwrap(), "nothing was seeded");
    }

    #[test]
    fn metadata_without_settings_enters_recovery_without_overwriting_data() {
        let db = Database::open_in_memory().unwrap();
        db.set_meta("permission_snapshot", "preserve-me").unwrap();

        assert_eq!(
            seed_first_run_defaults(&db).unwrap(),
            Initialization::MissingSettingsWithData
        );
        let recovery = match load_startup_configuration(&db).unwrap() {
            StartupConfiguration::RecoveryRequired(recovery) => recovery,
            StartupConfiguration::Ready(_) => {
                panic!("metadata without settings must not initialize automation")
            }
        };
        assert_eq!(
            recovery,
            crate::state::ConfigurationRecovery::SettingsUnreadable
        );

        let state = recovery_state(db, recovery);
        assert_eq!(*state.settings.lock_safe(), AppSettings::fail_closed());
        assert_eq!(
            startup_automation_plan(&state),
            StartupAutomationPlan {
                keyboard: false,
                drag_to_snap: false,
                drag_to_move: false,
                menu_bar_tidy: false,
            }
        );
        assert!(!state.db.settings_exist().unwrap(), "nothing was seeded");
        assert_eq!(state.db.count_hotkeys().unwrap(), 0);
        assert_eq!(state.db.count_modifier_rules().unwrap(), 0);
        assert_eq!(
            state.db.get_meta("permission_snapshot").unwrap().as_deref(),
            Some("preserve-me")
        );
    }

    #[test]
    fn placement_without_settings_enters_recovery_without_overwriting_data() {
        let db = Database::open_in_memory().unwrap();
        let placement = tomari_core::WindowPlacement {
            application: tomari_core::WindowApplication {
                bundle_id: "com.example.Editor".into(),
                name: "Editor".into(),
            },
            slot: tomari_core::PlacementSlot::Primary,
            frame: tomari_core::NormalizedRect::new(0.1, 0.2, 0.6, 0.7),
        };
        db.save_window_placement(&placement).unwrap();

        assert_eq!(
            seed_first_run_defaults(&db).unwrap(),
            Initialization::MissingSettingsWithData
        );
        let recovery = match load_startup_configuration(&db).unwrap() {
            StartupConfiguration::RecoveryRequired(recovery) => recovery,
            StartupConfiguration::Ready(_) => {
                panic!("placement without settings must not initialize automation")
            }
        };
        assert_eq!(
            recovery,
            crate::state::ConfigurationRecovery::SettingsUnreadable
        );

        let state = recovery_state(db, recovery);
        assert_eq!(*state.settings.lock_safe(), AppSettings::fail_closed());
        assert_eq!(
            startup_automation_plan(&state),
            StartupAutomationPlan {
                keyboard: false,
                drag_to_snap: false,
                drag_to_move: false,
                menu_bar_tidy: false,
            }
        );
        assert!(!state.db.settings_exist().unwrap(), "nothing was seeded");
        assert_eq!(state.db.count_hotkeys().unwrap(), 0);
        assert_eq!(state.db.count_modifier_rules().unwrap(), 0);
        assert_eq!(
            state
                .db
                .list_window_placements("com.example.Editor")
                .unwrap(),
            vec![placement]
        );
    }

    #[test]
    fn invalid_settings_json_keeps_every_startup_automation_path_off() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tomari.sqlite");
        {
            let db = Database::open(&path).unwrap();
            db.seed_defaults(
                &defaults::default_hotkeys(),
                &defaults::default_modifier_rules(),
                &AppSettings::fail_closed(),
            )
            .unwrap();
        }
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute(
                "UPDATE settings SET data = ?1 WHERE id = 1",
                ["{not valid settings json"],
            )
            .unwrap();
        }

        let db = Database::open(&path).unwrap();
        let recovery = match load_startup_configuration(&db).unwrap() {
            StartupConfiguration::RecoveryRequired(recovery) => recovery,
            StartupConfiguration::Ready(_) => {
                panic!("invalid settings JSON must not produce a runtime snapshot")
            }
        };
        assert_eq!(
            recovery,
            crate::state::ConfigurationRecovery::SettingsUnreadable
        );

        let state = recovery_state(db, recovery);
        assert_eq!(*state.settings.lock_safe(), AppSettings::fail_closed());
        assert_eq!(
            state.configuration_recovery(),
            Some(crate::state::ConfigurationRecovery::SettingsUnreadable)
        );
        assert_eq!(
            startup_automation_plan(&state),
            StartupAutomationPlan {
                keyboard: false,
                drag_to_snap: false,
                drag_to_move: false,
                menu_bar_tidy: false,
            }
        );
        assert!(state.shortcuts.lock_safe().is_empty());
        let engine = state.engine.lock_safe();
        assert!(!engine.has_caps_lock_rule());
        for rule in defaults::command_ime_rules() {
            assert!(!engine.contains_rule_id(&rule.id));
        }
        drop(engine);
        assert!(
            state.db.get_settings().is_err(),
            "entering recovery must not overwrite the unreadable row"
        );
        assert!(state.db.count_hotkeys().unwrap() > 0);
        assert!(state.db.count_modifier_rules().unwrap() > 0);
    }

    #[test]
    fn central_corruption_recovery_returns_an_unseeded_fail_closed_store() {
        use std::cell::Cell;

        let directory = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_root(directory.path());
        paths.ensure().unwrap();
        {
            let db = Database::open(&paths.db_path).unwrap();
            db.seed_defaults(
                &defaults::default_hotkeys(),
                &defaults::default_modifier_rules(),
                &AppSettings::default(),
            )
            .unwrap();
        }

        let notified = Cell::new(false);
        let fresh = recover_corrupt_database_with(
            &paths,
            &tomari_core::Error::DatabaseIntegrity("test fixture".into()),
            |_| notified.set(true),
        );
        assert!(notified.get());
        assert!(!fresh.settings_exist().unwrap());
        assert_eq!(fresh.count_hotkeys().unwrap(), 0);
        assert_eq!(fresh.count_modifier_rules().unwrap(), 0);
        assert!(recovery_markers::database_reset_required(&paths).unwrap());
        assert!(std::fs::read_dir(&paths.data_dir).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("tomari.sqlite.broken-")
        }));

        // The process can exit on the recovery screen. Re-open from disk and
        // prove the write-ahead marker still wins over first-run seeding.
        drop(fresh);
        let state = build_state(&paths);
        assert!(!state.first_run);
        assert_eq!(
            state.configuration_recovery(),
            Some(crate::state::ConfigurationRecovery::DatabaseReset)
        );
        assert!(!state.db.settings_exist().unwrap());
        assert_eq!(state.db.count_hotkeys().unwrap(), 0);
        assert_eq!(state.db.count_modifier_rules().unwrap(), 0);
        assert_eq!(*state.settings.lock_safe(), AppSettings::fail_closed());
        assert_eq!(
            startup_automation_plan(&state),
            StartupAutomationPlan {
                keyboard: false,
                drag_to_snap: false,
                drag_to_move: false,
                menu_bar_tidy: false,
            }
        );
    }
}
