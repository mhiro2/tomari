//! Domain value types shared across every Tomari crate and surfaced to the
//! frontend as camelCase JSON.

pub mod action;
pub mod keyboard;
pub mod settings;
pub mod window;

pub use action::{AppAction, ImeMode};
pub use keyboard::{Hotkey, KeySide, ModifierKey, ModifierRule};
pub use settings::{AppSettings, Language};
pub use window::{DisplayDirection, Rect, WindowPreset};

/// Generate a fresh random identifier for a new domain entity.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
