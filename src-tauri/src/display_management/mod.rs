//! Display management runs independently of Accessibility and keyboard hooks.

mod configuration;
mod controller;
#[cfg(target_os = "macos")]
mod dimming;
#[cfg(target_os = "macos")]
mod identify;
mod model;
#[cfg(target_os = "macos")]
mod native;
pub mod navigation;
pub mod profiles;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Instant;

use tauri::{AppHandle, Emitter, Manager};

use crate::locks::MutexExt;
use crate::state::AppState;
use configuration::ConfigurationRequest;
#[cfg(not(target_os = "macos"))]
use configuration::DisplaySetting;
use controller::DisplayController;
use model::*;

#[cfg(target_os = "macos")]
type Backend = native::NativeDisplayBackend;
#[cfg(not(target_os = "macos"))]
#[derive(Default)]
struct Backend;
#[cfg(not(target_os = "macos"))]
impl DisplayBackend for Backend {
    fn inventory(&mut self) -> Result<Vec<DisplayInfo>> {
        Err(unavailable())
    }
    fn apply_configuration(&mut self, _: &[DisplaySetting], _: bool) -> Result<()> {
        Err(unavailable())
    }
    fn set_brightness(&mut self, _: &str, _: f64) -> Result<()> {
        Err(unavailable())
    }
}
#[cfg(not(target_os = "macos"))]
fn unavailable() -> DisplayError {
    DisplayError::new(
        DisplayErrorCode::Unavailable,
        "display management requires macOS",
    )
}

pub struct DisplayState {
    controller: Mutex<DisplayController<Backend>>,
    watchdog_ready: AtomicBool,
    wake: Condvar,
}
impl Default for DisplayState {
    fn default() -> Self {
        Self {
            controller: Mutex::new(DisplayController::new(Backend::default())),
            watchdog_ready: AtomicBool::new(false),
            wake: Condvar::new(),
        }
    }
}

#[cfg(target_os = "macos")]
pub fn refresh_names() {
    identify::clear();
    native::refresh_names();
    dimming::refresh();
}

#[tauri::command]
pub fn identify_displays(app: AppHandle, display_ids: Vec<String>) -> Result<()> {
    if !app.state::<AppState>().lifecycle.is_running() {
        return Err(DisplayError::new(
            DisplayErrorCode::Busy,
            "the application is shutting down",
        ));
    }
    #[cfg(target_os = "macos")]
    {
        identify::show(&display_ids)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = display_ids;
        Err(unavailable())
    }
}

pub fn install(app: &AppHandle) {
    let handle = app.clone();
    let lifecycle = &app.state::<AppState>().lifecycle;
    let result = lifecycle.spawn_tracked("display-preview-watchdog", move |lifecycle| {
        let state = handle.state::<DisplayState>();
        let mut controller = state.controller.lock_safe();
        loop {
            if !lifecycle.is_running() {
                controller.restore();
                break;
            }
            if controller.expire(Instant::now()) {
                changed(&handle);
            }
            controller = match controller.next_timeout(Instant::now()) {
                Some(timeout) => {
                    state
                        .wake
                        .wait_timeout(controller, timeout)
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .0
                }
                None => state
                    .wake
                    .wait(controller)
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            };
        }
    });
    match result {
        Ok(ready) => app
            .state::<DisplayState>()
            .watchdog_ready
            .store(ready, Ordering::Release),
        Err(error) => tracing::error!(%error, "display preview watchdog could not start"),
    }
}

fn changed(app: &AppHandle) {
    let _ = app.emit("tomari:displays-changed", ());
}

#[tauri::command]
pub async fn get_displays(app: AppHandle) -> Result<DisplaySnapshot> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<DisplayState>()
            .controller
            .lock_safe()
            .snapshot(Instant::now)
    })
    .await
    .map_err(|e| DisplayError::new(DisplayErrorCode::System, e.to_string()))?
}

async fn mutate(
    app: AppHandle,
    operation: impl FnOnce(&mut DisplayController<Backend>) -> Result<DisplaySnapshot> + Send + 'static,
) -> Result<DisplaySnapshot> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _effect = state.lifecycle.runtime_effect().ok_or_else(|| {
            DisplayError::new(DisplayErrorCode::Busy, "the application is shutting down")
        })?;
        if state.configuration_recovery_required() {
            return Err(DisplayError::new(
                DisplayErrorCode::Unavailable,
                "repair the saved configuration before changing displays",
            ));
        }
        let displays = app.state::<DisplayState>();
        if !displays.watchdog_ready.load(Ordering::Acquire) {
            return Err(DisplayError::new(
                DisplayErrorCode::Unavailable,
                "display preview recovery is unavailable",
            ));
        }
        let result = operation(&mut displays.controller.lock_safe());
        displays.wake.notify_all();
        changed(&app);
        result
    })
    .await
    .map_err(|e| DisplayError::new(DisplayErrorCode::System, e.to_string()))?
}

#[tauri::command]
pub async fn preview_display_configuration(
    app: AppHandle,
    request: ConfigurationRequest,
) -> Result<DisplaySnapshot> {
    mutate(app, move |controller| {
        controller.preview(request, Instant::now)
    })
    .await
}
#[tauri::command]
pub async fn confirm_display_configuration(app: AppHandle, token: u64) -> Result<DisplaySnapshot> {
    mutate(app, move |controller| {
        controller.confirm(token, Instant::now)
    })
    .await
}
#[tauri::command]
pub async fn revert_display_configuration(app: AppHandle, token: u64) -> Result<DisplaySnapshot> {
    mutate(app, move |controller| {
        controller.revert(token, Instant::now)
    })
    .await
}
#[tauri::command]
pub async fn set_display_brightness(
    app: AppHandle,
    display_id: String,
    value: f64,
) -> Result<DisplaySnapshot> {
    mutate(app, move |controller| {
        controller.brightness(&display_id, value, Instant::now)
    })
    .await
}

pub fn has_dimming() -> bool {
    #[cfg(target_os = "macos")]
    {
        dimming::any_active()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[tauri::command]
pub fn reset_display_dimming(app: AppHandle) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if objc2::MainThreadMarker::new().is_none() {
            return Err(DisplayError::new(
                DisplayErrorCode::System,
                "software dimming requires the main thread",
            ));
        }
        dimming::clear();
        changed(&app);
        crate::tray::refresh(&app);
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err(unavailable())
    }
}

#[tauri::command]
pub fn set_display_dimming(app: AppHandle, display_id: String, amount: f64) -> Result<()> {
    let state = app.state::<AppState>();
    if !state.lifecycle.is_running() || state.configuration_recovery_required() {
        return Err(DisplayError::new(
            DisplayErrorCode::Unavailable,
            "display controls are unavailable while the app is recovering or closing",
        ));
    }
    // Never wait on the controller from the main thread. Quartz may be
    // delivering a screen-change notification while another operation owns it.
    let displays = app.state::<DisplayState>();
    let controller = displays.controller.try_lock_safe().ok_or_else(|| {
        DisplayError::new(
            DisplayErrorCode::Busy,
            "another display operation is running",
        )
    })?;
    if controller.has_pending() && amount != 0.0 {
        return Err(DisplayError::new(
            DisplayErrorCode::Busy,
            "finish the current display preview first",
        ));
    }
    #[cfg(target_os = "macos")]
    {
        dimming::apply(&display_id, amount)?;
        drop(controller);
        changed(&app);
        crate::tray::refresh(&app);
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (display_id, amount);
        Err(unavailable())
    }
}

pub fn prepare_shutdown(app: &AppHandle) {
    if let Some(state) = app.try_state::<DisplayState>() {
        // Pair notification with the predicate mutex so shutdown cannot fall
        // between the worker's lifecycle check and its indefinite idle wait.
        let _controller = state.controller.lock_safe();
        state.wake.notify_all();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = app.run_on_main_thread(|| {
            dimming::clear();
            identify::clear();
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
    }
}
