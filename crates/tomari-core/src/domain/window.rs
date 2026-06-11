//! Window-management value types. The geometry algorithm that turns a preset
//! into a concrete frame lives in the `tomari-window` crate; this module only
//! owns the shared data type so it can be referenced from settings and hotkeys.

use serde::{Deserialize, Serialize};

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
