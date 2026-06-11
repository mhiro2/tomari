//! `tomari-core` — the shared foundation for the Tomari menu-bar app.
//!
//! It owns the [`domain`] value types (which double as the JSON contract with
//! the frontend), the [`Error`] type used across crates, filesystem [`paths`]
//! and the SQLite [`Database`].

pub mod clock;
pub mod db;
pub mod defaults;
pub mod domain;
pub mod error;
pub mod external;
pub mod paths;

pub use db::{Database, ExportResult};
pub use error::{Error, Result};
pub use external::{DeepLinkError, ExternalAction, parse_deep_link};
pub use paths::AppPaths;

// Re-export the domain surface at the crate root for ergonomic downstream use.
pub use domain::{
    AppAction, AppSettings, CONFIG_FORMAT_VERSION, ConfigSnapshot, DisplayDirection, Hotkey,
    ImeMode, KeySide, Language, LaunchTarget, ModifierKey, ModifierRule, Rect, Theme, WindowPreset,
    new_id,
};
