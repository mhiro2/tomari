//! Retain a tray navigation request until the settings window acknowledges it.

use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager};

use crate::locks::MutexExt;

#[derive(Default)]
struct Request {
    revision: u64,
    pending: bool,
}

#[derive(Default)]
pub struct DisplayNavigation(Mutex<Request>);

impl DisplayNavigation {
    fn request(&self) {
        let mut request = self.0.lock_safe();
        request.revision = request.revision.wrapping_add(1).max(1);
        request.pending = true;
    }

    fn pending(&self) -> Option<u64> {
        let request = self.0.lock_safe();
        request.pending.then_some(request.revision)
    }

    fn acknowledge(&self, revision: u64) {
        let mut request = self.0.lock_safe();
        if request.revision == revision {
            request.pending = false;
        }
    }
}

pub fn open(app: &AppHandle) {
    app.state::<DisplayNavigation>().request();
    if let Err(error) = crate::actions::show_panel(app) {
        tracing::warn!(%error, "could not open display settings");
    }
    let _ = app.emit("tomari:open-displays", ());
}

#[tauri::command]
pub fn get_display_navigation(app: AppHandle) -> Option<u64> {
    app.state::<DisplayNavigation>().pending()
}

#[tauri::command]
pub fn acknowledge_display_navigation(app: AppHandle, revision: u64) {
    app.state::<DisplayNavigation>().acknowledge(revision);
}

#[tauri::command]
pub async fn open_system_display_settings() -> super::model::Result<()> {
    #[cfg(target_os = "macos")]
    {
        tauri::async_runtime::spawn_blocking(|| {
            crate::permissions::open_system_settings(
                "x-apple.systempreferences:com.apple.Displays-Settings.extension",
            )
        })
        .await
        .map_err(|error| {
            super::model::DisplayError::new(
                super::model::DisplayErrorCode::System,
                error.to_string(),
            )
        })?
        .map_err(|error| {
            super::model::DisplayError::new(
                super::model::DisplayErrorCode::System,
                error.to_string(),
            )
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(super::unavailable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_survives_reads_and_a_stale_acknowledgement() {
        let navigation = DisplayNavigation::default();
        assert_eq!(navigation.pending(), None);
        navigation.request();
        let first = navigation.pending().unwrap();
        assert_eq!(navigation.pending(), Some(first));
        navigation.request();
        let second = navigation.pending().unwrap();
        assert_ne!(first, second);
        navigation.acknowledge(first);
        assert_eq!(navigation.pending(), Some(second));
        navigation.acknowledge(second);
        assert_eq!(navigation.pending(), None);
    }
}
