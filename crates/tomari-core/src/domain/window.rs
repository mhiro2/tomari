//! Window-management value types. The geometry algorithm that turns a preset
//! into a concrete frame lives in the `tomari-window` crate; this module only
//! owns the shared data type so it can be referenced from settings and hotkeys.

use serde::{Deserialize, Serialize};

/// One of the two remembered homes an application can use. Two positions are
/// enough to cover the common "working" and "reference" contexts without
/// turning placement into a custom-layout editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlacementSlot {
    Primary,
    Secondary,
}

impl PlacementSlot {
    pub const ALL: [Self; 2] = [Self::Primary, Self::Secondary];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
        }
    }
}

/// A rectangle relative to a display's usable work area. Values normally sit
/// inside `0..=1`, but width/height may exceed one briefly while decoding bad
/// stored data; callers must validate before applying it to a real window.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl NormalizedRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Whether every component is finite and describes a non-empty rectangle
    /// fully inside the normalized work area.
    pub fn is_valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .into_iter()
            .all(f64::is_finite)
            && self.x >= 0.0
            && self.y >= 0.0
            && self.width > 0.0
            && self.height > 0.0
            && self.x + self.width <= 1.0 + f64::EPSILON
            && self.y + self.height <= 1.0 + f64::EPSILON
    }
}

/// Presentation-safe identity of the application owning a focused window.
/// The bundle identifier is the durable key; the localized name is a cached
/// label for settings UI and may change with the system language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowApplication {
    pub bundle_id: String,
    pub name: String,
}

/// One remembered position for an application.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowPlacement {
    pub application: WindowApplication,
    pub slot: PlacementSlot,
    pub frame: NormalizedRect,
}

/// A named target position/size for the focused window, relative to its screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowPreset {
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    TopLeftQuarter,
    TopRightQuarter,
    BottomLeftQuarter,
    BottomRightQuarter,
    LeftThird,
    CenterThird,
    RightThird,
    LeftTwoThirds,
    RightTwoThirds,
    Center,
    Maximize,
}

impl WindowPreset {
    /// All presets, in a sensible UI ordering.
    pub const ALL: [WindowPreset; 15] = [
        Self::LeftHalf,
        Self::RightHalf,
        Self::TopHalf,
        Self::BottomHalf,
        Self::TopLeftQuarter,
        Self::TopRightQuarter,
        Self::BottomLeftQuarter,
        Self::BottomRightQuarter,
        Self::LeftThird,
        Self::CenterThird,
        Self::RightThird,
        Self::LeftTwoThirds,
        Self::RightTwoThirds,
        Self::Center,
        Self::Maximize,
    ];

    /// The kebab-case token used in the `tomari://` URL scheme (e.g.
    /// `left-half`). This is the external API spelling, kept distinct from the
    /// serde `camelCase` form persisted in the database so the two can evolve
    /// independently.
    pub fn as_kebab(&self) -> &'static str {
        match self {
            Self::LeftHalf => "left-half",
            Self::RightHalf => "right-half",
            Self::TopHalf => "top-half",
            Self::BottomHalf => "bottom-half",
            Self::TopLeftQuarter => "top-left-quarter",
            Self::TopRightQuarter => "top-right-quarter",
            Self::BottomLeftQuarter => "bottom-left-quarter",
            Self::BottomRightQuarter => "bottom-right-quarter",
            Self::LeftThird => "left-third",
            Self::CenterThird => "center-third",
            Self::RightThird => "right-third",
            Self::LeftTwoThirds => "left-two-thirds",
            Self::RightTwoThirds => "right-two-thirds",
            Self::Center => "center",
            Self::Maximize => "maximize",
        }
    }

    /// Parse a kebab-case URL token back into a preset, or `None` if it names
    /// no known preset.
    pub fn from_kebab(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_kebab() == token)
    }

    /// A human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::LeftHalf => "Left Half",
            Self::RightHalf => "Right Half",
            Self::TopHalf => "Top Half",
            Self::BottomHalf => "Bottom Half",
            Self::TopLeftQuarter => "Top Left",
            Self::TopRightQuarter => "Top Right",
            Self::BottomLeftQuarter => "Bottom Left",
            Self::BottomRightQuarter => "Bottom Right",
            Self::LeftThird => "Left Third",
            Self::CenterThird => "Center Third",
            Self::RightThird => "Right Third",
            Self::LeftTwoThirds => "Left Two Thirds",
            Self::RightTwoThirds => "Right Two Thirds",
            Self::Center => "Center",
            Self::Maximize => "Maximize",
        }
    }
}

/// Which neighboring display to move the focused window to. Displays are
/// ordered left-to-right (then top-to-bottom) and wrap around.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DisplayDirection {
    Next,
    Prev,
}

impl DisplayDirection {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Next => "Next Display",
            Self::Prev => "Previous Display",
        }
    }

    /// The kebab-case token used in the `tomari://` URL scheme.
    pub fn as_kebab(&self) -> &'static str {
        match self {
            Self::Next => "next",
            Self::Prev => "prev",
        }
    }

    /// Parse a URL token (`next` / `prev`) into a direction.
    pub fn from_kebab(token: &str) -> Option<Self> {
        match token {
            "next" => Some(Self::Next),
            "prev" => Some(Self::Prev),
            _ => None,
        }
    }
}

/// A rectangle in screen coordinates (points, top-left origin).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_rect_rejects_non_finite_empty_and_out_of_bounds_values() {
        assert!(NormalizedRect::new(0.1, 0.2, 0.6, 0.7).is_valid());
        assert!(!NormalizedRect::new(f64::NAN, 0.0, 0.5, 0.5).is_valid());
        assert!(!NormalizedRect::new(0.0, 0.0, 0.0, 0.5).is_valid());
        assert!(!NormalizedRect::new(0.6, 0.0, 0.5, 0.5).is_valid());
    }
}
