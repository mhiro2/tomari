//! `tomari-core` — the shared foundation for the Tomari menu-bar app.
//!
//! It owns the [`domain`] value types (which double as the JSON contract with
//! the frontend), the [`Error`] type used across crates, filesystem [`paths`]
//! and the SQLite [`Database`].

pub mod backup;
pub mod clock;
pub mod db;
pub mod defaults;
pub mod domain;
pub mod error;
pub mod external;
pub mod paths;

pub use backup::{
    CONFIGURATION_BACKUP_FORMAT_VERSION, ConfigurationBackup, MAX_CONFIGURATION_BACKUP_BYTES,
    MAX_CONFIGURATION_BACKUP_RECORDS,
};
pub use db::{
    Database, DatabaseHealth, PersistedRowCounts, PersistedSettings, PersistedStatePreflight,
    StartupConfigurationReset,
};
pub use domain::settings::MAX_AUTO_COLLAPSE_SECS;
pub use error::{Error, Result};
pub use external::{DeepLinkError, ExternalAction, parse_deep_link};
pub use paths::AppPaths;

// Re-export the domain surface at the crate root for ergonomic downstream use.
pub use domain::{
    AppAction, AppSettings, DisplayDirection, Hotkey, ImeMode, KeySide, Language, ModifierKey,
    ModifierRule, NormalizedRect, PlacementSlot, Rect, WindowApplication, WindowPlacement,
    WindowPreset, new_id,
};
