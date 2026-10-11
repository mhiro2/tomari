//! Construct complete, reversible display transactions from a fresh inventory.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::model::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplaySetting {
    pub display_id: String,
    pub mode_id: i32,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConfigurationRequest {
    Mode {
        display_id: String,
        mode_id: i32,
    },
    Main {
        display_id: String,
    },
    Position {
        display_id: String,
        relative_to: String,
        placement: RelativePlacement,
    },
    #[serde(skip)]
    Profile {
        profile: tomari_core::DisplayProfile,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RelativePlacement {
    Left,
    Right,
    Above,
    Below,
}

impl ConfigurationRequest {
    pub fn display_id(&self) -> &str {
        match self {
            Self::Mode { display_id, .. }
            | Self::Main { display_id }
            | Self::Position { display_id, .. } => display_id,
            Self::Profile { profile } => profile
                .displays
                .first()
                .map_or("", |display| display.display_id.as_str()),
        }
    }
}

pub fn capture(displays: &[DisplayInfo]) -> Result<Vec<DisplaySetting>> {
    let mut ids = HashSet::new();
    let mut configuration = Vec::new();
    for display in displays {
        if display.mirrored {
            return Err(DisplayError::new(
                DisplayErrorCode::Mirrored,
                "configure mirrored displays together in System Settings",
            ));
        }
        if !ids.insert(&display.id) {
            return Err(DisplayError::new(
                DisplayErrorCode::Unavailable,
                "display identities are ambiguous",
            ));
        }
        let mode = display.current_mode.as_ref().ok_or_else(|| {
            DisplayError::new(
                DisplayErrorCode::ModeUnavailable,
                "the current display mode is unavailable",
            )
        })?;
        configuration.push(DisplaySetting {
            display_id: display.id.clone(),
            mode_id: mode.id,
            x: display.x,
            y: display.y,
        });
    }
    configuration.sort_by(|a, b| a.display_id.cmp(&b.display_id));
    Ok(configuration)
}

pub fn plan(
    displays: &[DisplayInfo],
    request: &ConfigurationRequest,
) -> Result<Vec<DisplaySetting>> {
    let mut configuration = capture(displays)?;
    if let ConfigurationRequest::Profile { profile } = request {
        return super::profiles::plan(displays, profile);
    }
    let display = find(displays, request.display_id())?;
    match request {
        ConfigurationRequest::Profile { .. } => {
            unreachable!("profiles are planned before individual changes")
        }
        ConfigurationRequest::Mode { mode_id, .. } => {
            if !display.modes.iter().any(|mode| mode.id == *mode_id) {
                return Err(DisplayError::new(
                    DisplayErrorCode::ModeUnavailable,
                    "the display mode is no longer available",
                ));
            }
            for setting in &mut configuration {
                if setting.display_id == display.id {
                    setting.mode_id = *mode_id;
                }
            }
        }
        ConfigurationRequest::Main { .. } => {
            // Quartz makes the display at the global origin the main display.
            for setting in &mut configuration {
                setting.x = coordinate(i64::from(setting.x) - i64::from(display.x))?;
                setting.y = coordinate(i64::from(setting.y) - i64::from(display.y))?;
            }
        }
        ConfigurationRequest::Position {
            relative_to,
            placement,
            ..
        } => {
            if relative_to == &display.id {
                return Err(DisplayError::new(
                    DisplayErrorCode::InvalidArrangement,
                    "a display cannot be positioned relative to itself",
                ));
            }
            let anchor = find(displays, relative_to)?;
            let (x, y) = relative_origin(display, anchor, *placement)?;
            for setting in &mut configuration {
                if setting.display_id == display.id {
                    setting.x = x;
                    setting.y = y;
                }
            }
            // Keep the existing main display at zero even when it was moved.
            if display.main {
                for setting in &mut configuration {
                    setting.x = coordinate(i64::from(setting.x) - i64::from(x))?;
                    setting.y = coordinate(i64::from(setting.y) - i64::from(y))?;
                }
            }
        }
    }
    Ok(configuration)
}

fn find<'a>(displays: &'a [DisplayInfo], id: &str) -> Result<&'a DisplayInfo> {
    displays
        .iter()
        .find(|display| display.id == id)
        .ok_or_else(|| {
            DisplayError::new(
                DisplayErrorCode::Disconnected,
                "the display is no longer connected",
            )
        })
}

fn relative_origin(
    display: &DisplayInfo,
    anchor: &DisplayInfo,
    placement: RelativePlacement,
) -> Result<(i32, i32)> {
    let x = i64::from(anchor.x);
    let y = i64::from(anchor.y);
    let centered_x = x + (i64::from(anchor.width) - i64::from(display.width)) / 2;
    let centered_y = y + (i64::from(anchor.height) - i64::from(display.height)) / 2;
    let (x, y) = match placement {
        RelativePlacement::Left => (x - i64::from(display.width), centered_y),
        RelativePlacement::Right => (x + i64::from(anchor.width), centered_y),
        RelativePlacement::Above => (centered_x, y - i64::from(display.height)),
        RelativePlacement::Below => (centered_x, y + i64::from(anchor.height)),
    };
    Ok((coordinate(x)?, coordinate(y)?))
}

fn coordinate(value: i64) -> Result<i32> {
    i32::try_from(value).map_err(|_| {
        DisplayError::new(
            DisplayErrorCode::InvalidArrangement,
            "display coordinates exceed the supported range",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(id: &str, x: i32, y: i32, width: u32, height: u32) -> DisplayInfo {
        let mode = DisplayMode {
            id: 1,
            width,
            height,
            pixel_width: width * 2,
            pixel_height: height * 2,
            refresh_rate: 60.0,
            hi_dpi: true,
        };
        DisplayInfo {
            connection_id: if id == "main" { 1 } else { 2 },
            id: id.into(),
            name: id.into(),
            built_in: false,
            main: x == 0 && y == 0,
            mirrored: false,
            x,
            y,
            width,
            height,
            current_mode: Some(mode.clone()),
            modes: vec![mode],
            brightness: None,
            software_dimming: 0.0,
        }
    }
    fn desktop() -> Vec<DisplayInfo> {
        vec![
            display("main", 0, 0, 2560, 1440),
            display("secondary", -1920, 0, 1920, 1080),
        ]
    }
    #[test]
    fn main_display_translation_preserves_relative_positions() {
        let result = plan(
            &desktop(),
            &ConfigurationRequest::Main {
                display_id: "secondary".into(),
            },
        )
        .unwrap();
        assert_eq!((result[0].x, result[0].y), (1920, 0));
        assert_eq!((result[1].x, result[1].y), (0, 0));
    }
    #[test]
    fn relative_arrangement_centers_the_adjacent_edges() {
        for (placement, expected) in [
            (RelativePlacement::Left, (-1920, 180)),
            (RelativePlacement::Right, (2560, 180)),
            (RelativePlacement::Above, (320, -1080)),
            (RelativePlacement::Below, (320, 1440)),
        ] {
            let result = plan(
                &desktop(),
                &ConfigurationRequest::Position {
                    display_id: "secondary".into(),
                    relative_to: "main".into(),
                    placement,
                },
            )
            .unwrap();
            assert_eq!((result[1].x, result[1].y), expected);
            assert_eq!((result[0].x, result[0].y), (0, 0));
        }
    }
    #[test]
    fn moving_the_main_display_keeps_its_origin_at_zero() {
        let result = plan(
            &desktop(),
            &ConfigurationRequest::Position {
                display_id: "main".into(),
                relative_to: "secondary".into(),
                placement: RelativePlacement::Left,
            },
        )
        .unwrap();
        assert_eq!((result[0].x, result[0].y), (0, 0));
        assert_eq!((result[1].x, result[1].y), (2560, 180));
    }
    #[test]
    fn stale_or_self_referencing_arrangements_are_rejected() {
        for reference in ["main", "missing"] {
            assert!(
                plan(
                    &desktop(),
                    &ConfigurationRequest::Position {
                        display_id: "main".into(),
                        relative_to: reference.into(),
                        placement: RelativePlacement::Below
                    }
                )
                .is_err()
            );
        }
    }
    #[test]
    fn capture_rejects_mirroring_missing_modes_and_ambiguous_identities() {
        let mut displays = desktop();
        displays[1].mirrored = true;
        assert_eq!(
            capture(&displays).unwrap_err().code,
            DisplayErrorCode::Mirrored
        );
        displays[1].mirrored = false;
        displays[1].id = "main".into();
        assert!(capture(&displays).is_err());
        displays[1].id = "secondary".into();
        displays[1].current_mode = None;
        assert_eq!(
            capture(&displays).unwrap_err().code,
            DisplayErrorCode::ModeUnavailable
        );
    }
    #[test]
    fn extreme_coordinates_are_rejected_without_overflow() {
        let mut displays = desktop();
        displays[0].x = i32::MAX;
        displays[1].x = i32::MIN;
        assert_eq!(
            plan(
                &displays,
                &ConfigurationRequest::Main {
                    display_id: "secondary".into()
                }
            )
            .unwrap_err()
            .code,
            DisplayErrorCode::InvalidArrangement
        );
    }
    #[test]
    fn configuration_capture_is_independent_of_enumeration_order() {
        let mut displays = desktop();
        let before = capture(&displays).unwrap();
        displays.reverse();
        assert_eq!(capture(&displays).unwrap(), before);
    }
    #[test]
    fn ipc_requests_use_the_frontend_discriminants_and_reject_extra_fields() {
        let request: ConfigurationRequest = serde_json::from_str(
            r#"{"kind":"position","displayId":"main","relativeTo":"secondary","placement":"left"}"#,
        )
        .unwrap();
        assert_eq!(request.display_id(), "main");
        assert!(
            serde_json::from_str::<ConfigurationRequest>(
                r#"{"kind":"main","displayId":"main","permanent":true}"#
            )
            .is_err()
        );
    }
}
