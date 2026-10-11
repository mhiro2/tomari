//! Explicitly saved display layouts and fresh-mode matching when recalled.

use std::time::Instant;

use tauri::{AppHandle, Manager};
use tomari_core::{Database, DisplayProfile, SavedDisplay, SavedDisplayMode};

use super::configuration::{self, ConfigurationRequest, DisplaySetting};
use super::model::*;
use super::{DisplayState, changed, mutate};
use crate::locks::MutexExt;
use crate::state::AppState;

fn storage_error(error: impl std::fmt::Display) -> DisplayError {
    DisplayError::new(DisplayErrorCode::ProfileStorage, error.to_string())
}

pub fn capture(displays: &[DisplayInfo], name: String) -> Result<DisplayProfile> {
    let configuration = configuration::capture(displays)?;
    let profile = DisplayProfile {
        id: tomari_core::new_id(),
        name: name.trim().to_string(),
        displays: configuration
            .into_iter()
            .map(|setting| {
                let mode = displays
                    .iter()
                    .find(|display| display.id == setting.display_id)
                    .and_then(|display| display.current_mode.as_ref())
                    .expect("capture validated every display's current mode");
                SavedDisplay {
                    display_id: setting.display_id,
                    x: setting.x,
                    y: setting.y,
                    mode: SavedDisplayMode {
                        width: mode.width,
                        height: mode.height,
                        pixel_width: mode.pixel_width,
                        pixel_height: mode.pixel_height,
                        refresh_rate: mode.refresh_rate,
                    },
                }
            })
            .collect(),
    };
    profile
        .validate()
        .map_err(|error| DisplayError::new(DisplayErrorCode::InvalidProfile, error.to_string()))?;
    Ok(profile)
}

pub fn plan(displays: &[DisplayInfo], profile: &DisplayProfile) -> Result<Vec<DisplaySetting>> {
    profile
        .validate()
        .map_err(|error| DisplayError::new(DisplayErrorCode::InvalidProfile, error.to_string()))?;
    if displays.len() != profile.displays.len() {
        return Err(DisplayError::new(
            DisplayErrorCode::ProfileUnavailable,
            "connect exactly the displays saved in this profile",
        ));
    }
    let mut configuration = Vec::new();
    for saved in &profile.displays {
        let display = displays
            .iter()
            .find(|display| display.id == saved.display_id)
            .ok_or_else(|| {
                DisplayError::new(
                    DisplayErrorCode::ProfileUnavailable,
                    "a saved display is not connected",
                )
            })?;
        let matches = |mode: &&DisplayMode| {
            mode.width == saved.mode.width
                && mode.height == saved.mode.height
                && mode.pixel_width == saved.mode.pixel_width
                && mode.pixel_height == saved.mode.pixel_height
                && (mode.refresh_rate - saved.mode.refresh_rate).abs() < 0.01
        };
        let mode = display
            .current_mode
            .as_ref()
            .filter(matches)
            .or_else(|| display.modes.iter().find(matches))
            .ok_or_else(|| {
                DisplayError::new(
                    DisplayErrorCode::ModeUnavailable,
                    "a saved mode is unavailable on this display",
                )
            })?;
        configuration.push(DisplaySetting {
            display_id: saved.display_id.clone(),
            mode_id: mode.id,
            x: saved.x,
            y: saved.y,
        });
    }
    configuration.sort_by(|a, b| a.display_id.cmp(&b.display_id));
    Ok(configuration)
}

#[tauri::command]
pub async fn get_display_profiles(app: AppHandle) -> Result<Vec<DisplayProfile>> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<AppState>()
            .db
            .list_display_profiles()
            .map_err(storage_error)
    })
    .await
    .map_err(storage_error)?
}

async fn edit_saved<T: Send + 'static>(
    app: AppHandle,
    operation: impl FnOnce(&AppState, &DisplayState) -> Result<T> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _guard = crate::commands::lock_config_mutation(&state).map_err(storage_error)?;
        let result = operation(&state, &app.state::<DisplayState>())?;
        changed(&app);
        Ok(result)
    })
    .await
    .map_err(storage_error)?
}

#[tauri::command]
pub async fn save_display_profile(app: AppHandle, name: String) -> Result<String> {
    edit_saved(app, move |state, displays| {
        let profile = displays
            .controller
            .lock_safe()
            .capture_profile(name, Instant::now)?;
        state
            .db
            .save_display_profile(&profile)
            .map_err(storage_error)?;
        Ok(profile.id)
    })
    .await
}

#[tauri::command]
pub async fn rename_display_profile(app: AppHandle, id: String, name: String) -> Result<()> {
    edit_saved(app, move |state, _| rename(&state.db, &id, name)).await
}

fn rename(db: &Database, id: &str, name: String) -> Result<()> {
    let mut profile = db
        .list_display_profiles()
        .map_err(storage_error)?
        .into_iter()
        .find(|profile| profile.id == id)
        .ok_or_else(|| {
            DisplayError::new(
                DisplayErrorCode::ProfileUnavailable,
                "the saved profile no longer exists",
            )
        })?;
    profile.name = name.trim().to_string();
    profile
        .validate()
        .map_err(|error| DisplayError::new(DisplayErrorCode::InvalidProfile, error.to_string()))?;
    db.save_display_profile(&profile).map_err(storage_error)
}

#[tauri::command]
pub async fn delete_display_profile(app: AppHandle, id: String) -> Result<()> {
    edit_saved(app, move |state, _| {
        state.db.delete_display_profile(&id).map_err(storage_error)
    })
    .await
}

#[tauri::command]
pub async fn clear_display_profiles(app: AppHandle) -> Result<()> {
    edit_saved(app, move |state, _| {
        state.db.clear_display_profiles().map_err(storage_error)
    })
    .await
}

#[tauri::command]
pub async fn preview_display_profile(app: AppHandle, id: String) -> Result<DisplaySnapshot> {
    let handle = app.clone();
    mutate(app, move |controller| {
        let profile = handle
            .state::<AppState>()
            .db
            .list_display_profiles()
            .map_err(storage_error)?
            .into_iter()
            .find(|profile| profile.id == id)
            .ok_or_else(|| {
                DisplayError::new(
                    DisplayErrorCode::ProfileUnavailable,
                    "the saved profile no longer exists",
                )
            })?;
        controller.preview(ConfigurationRequest::Profile { profile }, Instant::now)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop() -> Vec<DisplayInfo> {
        let mode = DisplayMode {
            id: 1,
            width: 1920,
            height: 1080,
            pixel_width: 3840,
            pixel_height: 2160,
            refresh_rate: 59.94,
            hi_dpi: true,
        };
        vec![DisplayInfo {
            connection_id: 1,
            id: "screen".into(),
            name: "Screen".into(),
            built_in: false,
            main: true,
            mirrored: false,
            x: 0,
            y: 0,
            width: mode.width,
            height: mode.height,
            current_mode: Some(mode.clone()),
            modes: vec![mode],
            brightness: Some(0.5),
            software_dimming: 0.2,
        }]
    }

    #[test]
    fn mode_identifiers_are_resolved_again_without_changing_saved_display_identity() {
        let mut displays = desktop();
        let profile = capture(&displays, " Work ".into()).unwrap();
        assert_eq!(profile.name, "Work");
        displays[0].modes[0].id = 99;
        displays[0].current_mode.as_mut().unwrap().id = 99;
        assert_eq!(plan(&displays, &profile).unwrap()[0].mode_id, 99);
        let json = serde_json::to_value(&profile).unwrap();
        assert!(json["displays"][0].get("brightness").is_none());
        assert!(json["displays"][0]["mode"].get("id").is_none());
    }

    #[test]
    fn profiles_require_the_exact_connected_display_set() {
        let mut displays = desktop();
        let profile = capture(&displays, "Work".into()).unwrap();
        assert_eq!(
            plan(&[], &profile).unwrap_err().code,
            DisplayErrorCode::ProfileUnavailable
        );
        displays[0].id = "replacement".into();
        assert_eq!(
            plan(&displays, &profile).unwrap_err().code,
            DisplayErrorCode::ProfileUnavailable
        );
    }

    #[test]
    fn matching_does_not_substitute_low_dpi_or_a_different_refresh_rate() {
        let original = desktop();
        let profile = capture(&original, "Work".into()).unwrap();
        for low_dpi in [true, false] {
            let mut displays = original.clone();
            if low_dpi {
                displays[0].modes[0].pixel_width = 1920;
            } else {
                displays[0].modes[0].refresh_rate = 60.0;
            }
            displays[0].current_mode = Some(displays[0].modes[0].clone());
            assert_eq!(
                plan(&displays, &profile).unwrap_err().code,
                DisplayErrorCode::ModeUnavailable
            );
        }
    }

    #[test]
    fn renaming_at_capacity_preserves_the_saved_layout_and_identity() {
        let db = Database::open_in_memory().unwrap();
        let mut profile = capture(&desktop(), "Work".into()).unwrap();
        for index in 0..tomari_core::MAX_DISPLAY_PROFILES {
            profile.id = index.to_string();
            db.save_display_profile(&profile).unwrap();
        }
        rename(&db, &profile.id, " Presentation ".into()).unwrap();
        profile.name = "Presentation".into();
        let saved = db.list_display_profiles().unwrap();
        assert_eq!(saved.len(), tomari_core::MAX_DISPLAY_PROFILES);
        assert_eq!(
            saved.iter().find(|saved| saved.id == profile.id),
            Some(&profile)
        );
    }

    #[test]
    fn invalid_or_missing_rename_does_not_change_saved_profiles() {
        let db = Database::open_in_memory().unwrap();
        let profile = capture(&desktop(), "Work".into()).unwrap();
        db.save_display_profile(&profile).unwrap();
        for name in [" ".to_string(), "a".repeat(81), "Work\nDesk".to_string()] {
            assert_eq!(
                rename(&db, &profile.id, name).unwrap_err().code,
                DisplayErrorCode::InvalidProfile
            );
        }
        assert_eq!(
            rename(&db, "removed", "Missing".into()).unwrap_err().code,
            DisplayErrorCode::ProfileUnavailable
        );
        assert_eq!(db.list_display_profiles().unwrap(), vec![profile]);
    }

    #[test]
    fn raw_ipc_cannot_supply_a_profile_instead_of_a_stored_identifier() {
        let profile = capture(&desktop(), "Work".into()).unwrap();
        let value = serde_json::json!({"kind": "profile", "profile": profile});
        assert!(serde_json::from_value::<ConfigurationRequest>(value).is_err());
    }
}
