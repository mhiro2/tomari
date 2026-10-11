//! Saved display configurations use stable identities and mode descriptions.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const MAX_DISPLAY_PROFILES: usize = 32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DisplayProfile {
    pub id: String,
    pub name: String,
    pub displays: Vec<SavedDisplay>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedDisplay {
    pub display_id: String,
    pub x: i32,
    pub y: i32,
    pub mode: SavedDisplayMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedDisplayMode {
    pub width: u32,
    pub height: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub refresh_rate: f64,
}

impl DisplayProfile {
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty() || self.id.len() > 128 {
            return Err(Error::invalid("displayProfile.id", "invalid identifier"));
        }
        if self.name.trim().is_empty()
            || self.name.trim() != self.name
            || self.name.chars().count() > 80
            || self.name.chars().any(char::is_control)
        {
            return Err(Error::invalid(
                "displayProfile.name",
                "must contain 1–80 printable characters without surrounding whitespace",
            ));
        }
        if self.displays.is_empty() || self.displays.len() > 16 {
            return Err(Error::invalid(
                "displayProfile.displays",
                "must contain 1–16 displays",
            ));
        }
        let mut ids = HashSet::new();
        let mut origins = 0;
        for display in &self.displays {
            if display.display_id.is_empty()
                || display.display_id.len() > 128
                || !ids.insert(&display.display_id)
            {
                return Err(Error::invalid(
                    "displayProfile.displays",
                    "invalid or repeated display identity",
                ));
            }
            origins += usize::from(display.x == 0 && display.y == 0);
            let mode = &display.mode;
            if [mode.width, mode.height, mode.pixel_width, mode.pixel_height]
                .into_iter()
                .any(|size| size == 0 || size > 65536)
                || !mode.refresh_rate.is_finite()
                || !(0.0..=1000.0).contains(&mode.refresh_rate)
            {
                return Err(Error::invalid(
                    "displayProfile.mode",
                    "invalid display mode dimensions or refresh rate",
                ));
            }
        }
        if origins != 1 {
            return Err(Error::invalid(
                "displayProfile.displays",
                "exactly one main display must be at the origin",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn sample() -> DisplayProfile {
    DisplayProfile {
        id: "profile".into(),
        name: "Work".into(),
        displays: vec![SavedDisplay {
            display_id: "display".into(),
            x: 0,
            y: 0,
            mode: SavedDisplayMode {
                width: 1920,
                height: 1080,
                pixel_width: 3840,
                pixel_height: 2160,
                refresh_rate: 59.94,
            },
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ambiguous_identities_invalid_modes_and_missing_main_display() {
        assert!(sample().validate().is_ok());
        let mut profile = sample();
        profile.displays.push(profile.displays[0].clone());
        assert!(profile.validate().is_err());
        let mut profile = sample();
        profile.displays[0].x = 100;
        assert!(profile.validate().is_err());
        let mut profile = sample();
        profile.displays[0].mode.refresh_rate = f64::NAN;
        assert!(profile.validate().is_err());
        let mut profile = sample();
        profile.displays[0].mode.pixel_width = 0;
        assert!(profile.validate().is_err());
        let mut profile = sample();
        profile.name = " Work ".into();
        assert!(profile.validate().is_err());
    }
}
