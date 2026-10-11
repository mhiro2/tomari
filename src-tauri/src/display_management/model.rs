//! Serializable display inventory and reversible configuration requests.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayMode {
    pub id: i32,
    pub width: u32,
    pub height: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub refresh_rate: f64,
    pub hi_dpi: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayInfo {
    pub connection_id: u32,
    pub id: String,
    pub name: String,
    pub built_in: bool,
    pub main: bool,
    pub mirrored: bool,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub current_mode: Option<DisplayMode>,
    pub modes: Vec<DisplayMode>,
    pub brightness: Option<f64>,
    pub software_dimming: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplaySnapshot {
    pub displays: Vec<DisplayInfo>,
    pub pending: Option<PreviewInfo>,
    pub notice: Option<DisplayNotice>,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInfo {
    pub token: u64,
    pub display_id: String,
    pub remaining_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DisplayNotice {
    Reverted,
    RestoreFailed,
    DisplayChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DisplayErrorCode {
    Unavailable,
    Disconnected,
    ModeUnavailable,
    Mirrored,
    Busy,
    PreviewExpired,
    InvalidArrangement,
    InvalidBrightness,
    InvalidDimming,
    BrightnessUnavailable,
    System,
    RefreshRequired,
    InvalidProfile,
    ProfileUnavailable,
    ProfileStorage,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayError {
    pub code: DisplayErrorCode,
    pub message: String,
}

impl DisplayError {
    pub fn new(code: DisplayErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, DisplayError>;

/// The operating system owns the inventory; IPC identifiers never authorize
/// an unchecked display mutation. Implementations resolve each request again.
pub trait DisplayBackend: Send {
    fn inventory(&mut self) -> Result<Vec<DisplayInfo>>;
    fn apply_configuration(
        &mut self,
        settings: &[super::configuration::DisplaySetting],
        permanent: bool,
    ) -> Result<()>;
    fn set_brightness(&mut self, display_id: &str, value: f64) -> Result<()>;
}
